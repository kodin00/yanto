//! `/users`: Owner-only Member management (invites, access, status, reset links, deletion).
//! The service enforces Owner-only; a Member who reaches these routes gets a 403 page.

use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Form, Router};
use serde::Deserialize;
use std::collections::BTreeMap;
use yanto_core::CoreError;
use yanto_core::actor::Permission;
use yanto_core::db::repo::users::UserStatus;
use yanto_core::service::users::{GrantInput, UserDto};

use super::session::Session;
use super::view::{DashError, PageCtx, escape, fmt_opt_ts, fmt_ts, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users", get(index).post(create))
        .route("/users/{id}", get(detail).delete(delete))
        .route("/users/{id}/access", put(update_access))
        .route("/users/{id}/status", patch(set_status))
        .route("/users/{id}/reset-link", post(reset_link))
}

struct UserRowView {
    id: String,
    username: String,
    role: &'static str,
    status: &'static str,
    last_login: String,
    projects: usize,
}

impl From<&UserDto> for UserRowView {
    fn from(u: &UserDto) -> Self {
        Self {
            id: u.id.clone(),
            username: u.username.clone(),
            role: u.role,
            status: u.status,
            last_login: fmt_opt_ts(u.last_login_at),
            projects: u.grants.len(),
        }
    }
}

#[derive(Template)]
#[template(path = "users/index.html")]
struct IndexPage {
    ctx: PageCtx,
    users: Vec<UserRowView>,
    error: Option<String>,
    new_username: String,
}

#[derive(Template)]
#[template(path = "users/link.html")]
struct LinkPage {
    ctx: PageCtx,
    username: String,
    url: String,
    expires: String,
    what: &'static str,
}

struct GrantView {
    project_id: String,
    /// `Name (slug)` when the Project still exists.
    label: String,
    perms: Vec<(&'static str, bool)>,
}

#[derive(Template)]
#[template(path = "users/detail.html")]
struct DetailPage {
    ctx: PageCtx,
    user: UserRowView,
    grants: Vec<GrantView>,
    /// Projects the Member has no row for yet, for the "add" picker.
    choices: Vec<(String, String)>,
    all_permissions: Vec<&'static str>,
    created: String,
    is_owner: bool,
    is_disabled: bool,
}

async fn list_users(s: &AppState, session: &Session) -> Result<Vec<UserDto>, CoreError> {
    s.services.users.list(&session.0.actor).await
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let users = list_users(&s, &session)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&IndexPage {
        ctx: PageCtx::new(&session, "Users", "/users"),
        users: users.iter().map(Into::into).collect(),
        error: None,
        new_username: String::new(),
    }))
}

#[derive(Deserialize)]
struct CreateForm {
    username: String,
}

async fn create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<CreateForm>,
) -> Result<Response, DashError> {
    match s
        .services
        .users
        .create(&session.0.actor, &f.username, &[])
        .await
    {
        Ok(out) => Ok(render(&LinkPage {
            ctx: PageCtx::new(&session, "Invite link", "/users"),
            username: out.user.username,
            url: out.invite.url.expose().to_owned(),
            expires: fmt_ts(out.invite.expires_at),
            what: "Invitation",
        })),
        Err(e)
            if matches!(
                e.kind,
                yanto_core::ErrorKind::Validation(_) | yanto_core::ErrorKind::Conflict
            ) =>
        {
            let users = list_users(&s, &session)
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(
                status_for(&e.kind),
                &IndexPage {
                    ctx: PageCtx::new(&session, "Users", "/users"),
                    users: users.iter().map(Into::into).collect(),
                    error: Some(public_message(&e)),
                    new_username: f.username,
                },
            ))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}

async fn detail(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let user = s
        .services
        .users
        .get(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let projects = s
        .services
        .projects
        .list(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let label_of = |id: &str| {
        projects
            .iter()
            .find(|p| p.id == id)
            .map_or_else(|| id.to_owned(), |p| format!("{} ({})", p.name, p.slug))
    };
    let choices = projects
        .iter()
        .filter(|p| !user.grants.iter().any(|g| g.project_id == p.id))
        .map(|p| (p.id.clone(), format!("{} ({})", p.name, p.slug)))
        .collect();
    let grants = user
        .grants
        .iter()
        .map(|g| GrantView {
            label: label_of(&g.project_id),
            project_id: g.project_id.clone(),
            perms: Permission::ALL
                .iter()
                .map(|p| (p.as_str(), g.permissions.iter().any(|x| x == p.as_str())))
                .collect(),
        })
        .collect();
    Ok(render(&DetailPage {
        ctx: PageCtx::new(&session, format!("User {}", user.username), "/users"),
        is_owner: user.role == "owner",
        is_disabled: user.status == "disabled",
        created: fmt_ts(user.created_at),
        user: (&user).into(),
        grants,
        choices,
        all_permissions: Permission::ALL.iter().map(|p| p.as_str()).collect(),
    }))
}

/// Parse the access editor form: `keep=<project>` rows (uncheck to remove),
/// `perm=<project>|<permission>` checkboxes, and an optional `new_project` row.
fn parse_access_form(body: &[u8]) -> Result<Vec<GrantInput>, CoreError> {
    let mut grants: BTreeMap<String, Vec<Permission>> = BTreeMap::new();
    let mut perms: Vec<(String, String)> = Vec::new();
    let mut new_project = String::new();
    let mut new_perms: Vec<String> = Vec::new();
    for (k, v) in form_urlencoded::parse(body) {
        match k.as_ref() {
            "keep" => {
                grants.entry(v.into_owned()).or_default();
            }
            "perm" => {
                if let Some((p, perm)) = v.split_once('|') {
                    perms.push((p.to_owned(), perm.to_owned()));
                }
            }
            "new_project" => new_project = v.trim().to_owned(),
            "new_perm" => new_perms.push(v.into_owned()),
            _ => {}
        }
    }
    for (p, perm) in perms {
        if let Some(list) = grants.get_mut(&p) {
            list.push(perm.parse()?);
        }
    }
    if !new_project.is_empty() {
        let list = grants.entry(new_project).or_default();
        for p in new_perms {
            list.push(p.parse()?);
        }
    }
    Ok(grants
        .into_iter()
        .map(|(project_id, permissions)| GrantInput {
            project_id,
            permissions,
        })
        .collect())
}

/// htmx `hx-put`: replace the Member's grants; returns a status fragment.
async fn update_access(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, DashError> {
    let run = async {
        let grants = parse_access_form(&body)?;
        s.services
            .users
            .update_access(&session.0.actor, &id, &grants)
            .await
    };
    run.await.map_err(|e| DashError::new(e, &headers))?;
    let mut res = Html("<span class=\"ok\">Saved.</span>").into_response();
    res.headers_mut()
        .insert("hx-trigger", HeaderValue::from_static("access-saved"));
    Ok(res)
}

#[derive(Deserialize)]
struct StatusForm {
    status: String,
}

/// htmx `hx-patch`: disable or re-enable, then refresh the page.
async fn set_status(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<StatusForm>,
) -> Result<Response, DashError> {
    let status = UserStatus::parse(&f.status).unwrap_or(UserStatus::Invited);
    s.services
        .users
        .set_status(&session.0.actor, &id, status)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let mut res = Html("").into_response();
    res.headers_mut()
        .insert("hx-refresh", HeaderValue::from_static("true"));
    Ok(res)
}

/// htmx `hx-post`: issue a reset link and show it once.
async fn reset_link(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let link = s
        .services
        .users
        .reset_link(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let mut res = Html(format!(
        "<div class=\"secret\"><p>Reset link for <strong>{}</strong> (valid until {}, shown once, \
         all sessions were revoked):</p><code class=\"link\">{}</code></div>",
        escape(&link.username),
        fmt_ts(link.expires_at),
        escape(link.url.expose())
    ))
    .into_response();
    res.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    Ok(res)
}

/// htmx `hx-delete`: delete and go back to the list.
async fn delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .users
        .delete(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let mut res = Html("").into_response();
    res.headers_mut()
        .insert("hx-redirect", HeaderValue::from_static("/users"));
    Ok(res)
}
