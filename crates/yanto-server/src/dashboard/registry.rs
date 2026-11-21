//! `/registry`: the Hosted Registry (docs/rewrite/04 section 4). Owner only: status, enable and
//! disable, registry users, the repo/tag browser, tag delete, garbage collection and the
//! remote-push guidance. Registry passwords are shown once, on the page that created them.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::secrets::Secret;
use yanto_core::service::fmt_bytes;
use yanto_core::service::registry::{
    DELETE_DATA_PHRASE, EnableInput, GcState, GcStatus, Health, RegistryStatus,
};
use yanto_core::{CoreError, ErrorKind};

use super::session::Session;
use super::view::{DashError, PageCtx, escape, fmt_opt_ts, fmt_ts, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/registry", get(index))
        .route("/registry/enable", post(enable))
        .route("/registry/disable", post(disable))
        .route("/registry/restart", post(restart))
        .route("/registry/rerender", post(rerender))
        .route("/registry/settings", post(settings))
        .route("/registry/remote-push", get(remote_push))
        .route("/registry/users", get(users_fragment).post(user_create))
        .route(
            "/registry/users/{id}",
            delete(user_delete).put(user_password),
        )
        .route("/registry/repos", get(repos_fragment))
        .route("/registry/repos/{name}/tags", get(tags_fragment))
        .route(
            "/registry/repos/{name}/tags/{tag}",
            get(tag_confirm).delete(tag_delete),
        )
        .route("/registry/gc", get(gc_fragment).post(gc_start))
        .route("/registry/gc/preview", post(gc_preview))
}

/// Percent-encode everything but unreserved characters, so `team/app` fits one path segment.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

struct UserView {
    id: String,
    username: String,
    created: String,
}

/// A secret shown once.
struct Revealed {
    username: String,
    password: String,
}

#[derive(Template)]
#[template(path = "registry/index.html")]
struct IndexPage {
    ctx: PageCtx,
    st: RegistryStatus,
    health_label: String,
    health_ok: bool,
    auth_mismatch: bool,
    container_line: String,
    disk: String,
    users: Vec<UserView>,
    gc_html: String,
    error: Option<String>,
    revealed: Option<Revealed>,
    delete_phrase: &'static str,
    username_value: String,
}

#[derive(Default)]
struct Notice {
    error: Option<String>,
    revealed: Option<Revealed>,
    username: String,
}

async fn page(s: &AppState, session: &Session, n: Notice) -> Result<IndexPage, CoreError> {
    let actor = &session.0.actor;
    let st = s.services.registry.status(actor).await?;
    let users = s.services.registry.users_list(actor).await?;
    let gc = s.services.registry.gc_status(actor).await?;
    let (health_label, health_ok, auth_mismatch) = match &st.health {
        Health::Disabled => ("disabled".to_owned(), false, false),
        Health::Healthy => ("healthy".to_owned(), true, false),
        Health::Down(d) => (format!("down: {d}"), false, false),
        Health::AuthMismatch => ("auth mismatch".to_owned(), false, true),
    };
    let container_line = st
        .container
        .as_ref()
        .map_or_else(|| "no container".to_owned(), |c| c.status.clone());
    let disk = st.disk_bytes.map_or_else(|| "-".to_owned(), fmt_bytes);
    Ok(IndexPage {
        ctx: PageCtx::new(session, "Registry", "/registry"),
        health_label,
        health_ok,
        auth_mismatch,
        container_line,
        disk,
        users: users
            .into_iter()
            .map(|u| UserView {
                created: fmt_ts(u.created_at),
                id: u.id,
                username: u.username,
            })
            .collect(),
        gc_html: gc_html(&gc),
        st,
        error: n.error,
        revealed: n.revealed,
        delete_phrase: DELETE_DATA_PHRASE,
        username_value: n.username,
    })
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let p = page(&s, &session, Notice::default())
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&p))
}

/// Validation and conflict errors re-render the page with the message; anything else is an error page.
async fn done(
    s: &AppState,
    session: &Session,
    headers: &HeaderMap,
    r: Result<Notice, CoreError>,
) -> Result<Response, DashError> {
    let (notice, status) = match r {
        Ok(n) => {
            if n.revealed.is_none() && n.error.is_none() {
                return Ok(axum::response::Redirect::to("/registry").into_response());
            }
            (n, StatusCode::OK)
        }
        Err(e)
            if matches!(
                e.kind,
                ErrorKind::Validation(_) | ErrorKind::Conflict | ErrorKind::Unavailable
            ) =>
        {
            let status = status_for(&e.kind);
            (
                Notice {
                    error: Some(public_message(&e)),
                    ..Notice::default()
                },
                status,
            )
        }
        Err(e) => return Err(DashError::new(e, headers)),
    };
    let p = page(s, session, notice)
        .await
        .map_err(|e| DashError::new(e, headers))?;
    Ok(render_status(status, &p))
}

#[derive(Deserialize)]
struct EnableForm {
    #[serde(default)]
    listen_port: String,
}

async fn enable(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<EnableForm>,
) -> Result<Response, DashError> {
    let r = async {
        let port = match f.listen_port.trim() {
            "" => None,
            p => Some(p.parse::<u16>().map_err(|_| {
                CoreError::validation(
                    "listen_port",
                    "the port must be a number from 1024 to 65535",
                )
            })?),
        };
        s.services
            .registry
            .enable(&session.0.actor, &EnableInput { listen_port: port })
            .await
            .map(|_| Notice::default())
    }
    .await;
    done(&s, &session, &headers, r).await
}

#[derive(Deserialize)]
struct DisableForm {
    #[serde(default)]
    delete_data: String,
}

async fn disable(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<DisableForm>,
) -> Result<Response, DashError> {
    let r = s
        .services
        .registry
        .disable(&session.0.actor, Some(&f.delete_data))
        .await
        .map(|_| Notice::default());
    done(&s, &session, &headers, r).await
}

async fn restart(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let r = s
        .services
        .registry
        .restart(&session.0.actor)
        .await
        .map(|_| Notice::default());
    done(&s, &session, &headers, r).await
}

async fn rerender(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let r = s
        .services
        .registry
        .rerender(&session.0.actor)
        .await
        .map(|_| Notice::default());
    done(&s, &session, &headers, r).await
}

#[derive(Deserialize)]
struct SettingsForm {
    #[serde(default)]
    public_host: String,
}

async fn settings(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<SettingsForm>,
) -> Result<Response, DashError> {
    let r = s
        .services
        .registry
        .settings_update(&session.0.actor, Some(&f.public_host))
        .await
        .map(|_| Notice::default());
    done(&s, &session, &headers, r).await
}

// ---- users ---------------------------------------------------------------------------

async fn users_fragment(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let users = s
        .services
        .registry
        .users_list(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let mut html = String::from("<ul>");
    for u in users {
        html.push_str(&format!("<li>{}</li>", escape(&u.username)));
    }
    html.push_str("</ul>");
    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct UserForm {
    username: String,
    #[serde(default)]
    password: String,
}

fn pw_opt(p: &str) -> Option<Secret> {
    (!p.is_empty()).then(|| Secret::new(p))
}

async fn user_create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<UserForm>,
) -> Result<Response, DashError> {
    let r = s
        .services
        .registry
        .user_create(
            &session.0.actor,
            f.username.trim(),
            pw_opt(&f.password).as_ref(),
        )
        .await
        .map(|out| Notice {
            revealed: out.generated_password.as_ref().map(|p| Revealed {
                username: out.user.username.clone(),
                password: p.expose().to_owned(),
            }),
            ..Notice::default()
        });
    let r = match r {
        Err(e) if matches!(e.kind, ErrorKind::Validation(_) | ErrorKind::Conflict) => {
            // Keep what was typed so a validation error is easy to fix.
            let status = status_for(&e.kind);
            let p = page(
                &s,
                &session,
                Notice {
                    error: Some(public_message(&e)),
                    username: f.username.clone(),
                    ..Notice::default()
                },
            )
            .await
            .map_err(|e| DashError::new(e, &headers))?;
            return Ok(render_status(status, &p));
        }
        other => other,
    };
    done(&s, &session, &headers, r).await
}

#[derive(Deserialize, Default)]
struct PasswordForm {
    #[serde(default)]
    password: String,
}

async fn user_password(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<PasswordForm>,
) -> Result<Response, DashError> {
    let generated = s
        .services
        .registry
        .user_password_set(&session.0.actor, &id, pw_opt(&f.password).as_ref())
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let msg = match generated {
        Some(pw) => format!(
            "New password: <code>{}</code> (shown once)",
            escape(pw.expose())
        ),
        None => "Password changed.".to_owned(),
    };
    let mut res = Html(format!("<div class=\"toast-msg\">{msg}</div>")).into_response();
    res.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    Ok(res)
}

async fn user_delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .registry
        .user_delete(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let mut res = (StatusCode::OK, Html("")).into_response();
    res.headers_mut()
        .insert("hx-refresh", HeaderValue::from_static("true"));
    Ok(res)
}

// ---- browse --------------------------------------------------------------------------

#[derive(Deserialize)]
struct PageQuery {
    last: Option<String>,
}

struct RepoRow {
    name: String,
    enc: String,
}

#[derive(Template)]
#[template(path = "registry/_repos.html")]
struct ReposFragment {
    repos: Vec<RepoRow>,
    next_enc: Option<String>,
    image_host: String,
}

async fn repos_fragment(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Result<Response, DashError> {
    let page = s
        .services
        .registry
        .repos_list(&session.0.actor, q.last.as_deref(), None)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let st = s
        .services
        .registry
        .status(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&ReposFragment {
        repos: page
            .repos
            .into_iter()
            .map(|n| RepoRow {
                enc: enc(&n),
                name: n,
            })
            .collect(),
        next_enc: page.next.map(|n| enc(&n)),
        image_host: st.image_host,
    }))
}

struct TagRow {
    tag: String,
    tag_enc: String,
    digest: String,
    short_digest: String,
    size: String,
    kind: String,
}

#[derive(Template)]
#[template(path = "registry/_tags.html")]
struct TagsFragment {
    repo: String,
    repo_enc: String,
    tags: Vec<TagRow>,
    next_enc: Option<String>,
    image_host: String,
}

async fn tags_fragment(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response, DashError> {
    let reg = &s.services.registry;
    let page = reg
        .tags_list(&session.0.actor, &name, q.last.as_deref(), None)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let st = reg
        .status(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&TagsFragment {
        repo_enc: enc(&name),
        repo: name,
        tags: page
            .tags
            .into_iter()
            .map(|t| TagRow {
                tag_enc: enc(&t.tag),
                short_digest: t.digest.chars().take(19).collect(),
                size: t.size.map_or_else(|| "-".to_owned(), fmt_bytes),
                kind: match t.children {
                    Some(n) => format!("index ({n} platforms)"),
                    None => "image".to_owned(),
                },
                digest: t.digest,
                tag: t.tag,
            })
            .collect(),
        next_enc: page.next.map(|n| enc(&n)),
        image_host: st.image_host,
    }))
}

#[derive(Template)]
#[template(path = "registry/_delete_confirm.html")]
struct DeleteConfirm {
    repo: String,
    repo_enc: String,
    tag: String,
    tag_enc: String,
    digest: String,
    also: Vec<String>,
}

async fn tag_confirm(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path((name, tag)): Path<(String, String)>,
) -> Result<Response, DashError> {
    let p = s
        .services
        .registry
        .tag_delete_preview(&session.0.actor, &name, &tag)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&DeleteConfirm {
        repo_enc: enc(&p.repo),
        tag_enc: enc(&p.tag),
        repo: p.repo,
        tag: p.tag,
        digest: p.digest,
        also: p.also_removed,
    }))
}

async fn tag_delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path((name, tag)): Path<(String, String)>,
) -> Result<Response, DashError> {
    let p = s
        .services
        .registry
        .tag_delete(&session.0.actor, &name, &tag)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let mut removed = vec![p.tag.clone()];
    removed.extend(p.also_removed.clone());
    let msg = format!(
        "Deleted {}:{} ({}). Disk space is reclaimed by garbage collection.",
        p.repo,
        removed.join(", "),
        p.digest
    );
    let mut res = Html(format!("<div class=\"toast-msg\">{}</div>", escape(&msg))).into_response();
    res.headers_mut()
        .insert("hx-trigger", HeaderValue::from_static("registry-changed"));
    Ok(res)
}

// ---- garbage collection --------------------------------------------------------------

fn gc_html(g: &GcStatus) -> String {
    let (label, class) = match g.state {
        GcState::Idle => ("No collection has run since Yanto started.", ""),
        GcState::Running => (
            "Collection running; pushes are rejected until it finishes.",
            "running",
        ),
        GcState::Done => ("Last collection finished.", "ok"),
        GcState::Failed => ("Last collection failed.", "error"),
    };
    // While running the fragment re-polls itself; the swap stops once the state changes.
    let poll = if g.state == GcState::Running {
        " hx-get=\"/registry/gc\" hx-trigger=\"every 2s\" hx-swap=\"outerHTML\""
    } else {
        ""
    };
    let mut h = format!(
        "<div id=\"gc-status\"{poll}><p class=\"{class}\">{}</p>",
        escape(label)
    );
    if let Some(t) = g.started_at {
        h.push_str(&format!(
            "<p class=\"muted\">Started {}",
            escape(&fmt_ts(t))
        ));
        if g.finished_at.is_some() {
            h.push_str(&format!(
                ", finished {}",
                escape(&fmt_opt_ts(g.finished_at))
            ));
        }
        if let Some(b) = g.reclaimed_bytes {
            h.push_str(&format!(", reclaimed {}", escape(&fmt_bytes(b))));
        }
        h.push_str("</p>");
    }
    if let Some(e) = &g.error {
        h.push_str(&format!("<p class=\"error\">{}</p>", escape(e)));
    }
    if !g.output.is_empty() {
        h.push_str(&format!("<pre class=\"log\">{}</pre>", escape(&g.output)));
    }
    h.push_str("</div>");
    h
}

async fn gc_fragment(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let g = s
        .services
        .registry
        .gc_status(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(Html(gc_html(&g)).into_response())
}

#[derive(Deserialize, Default)]
struct GcForm {
    #[serde(default)]
    delete_untagged: Option<String>,
}

async fn gc_start(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<GcForm>,
) -> Result<Response, DashError> {
    let g = s
        .services
        .registry
        .gc_start(&session.0.actor, f.delete_untagged.is_some())
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(Html(gc_html(&g)).into_response())
}

async fn gc_preview(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<GcForm>,
) -> Result<Response, DashError> {
    let p = s
        .services
        .registry
        .gc_preview(&session.0.actor, f.delete_untagged.is_some())
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(Html(format!(
        "<pre class=\"log\">{}</pre>",
        escape(if p.output.trim().is_empty() {
            "(the dry run printed nothing)"
        } else {
            &p.output
        })
    ))
    .into_response())
}

// ---- remote push guidance ------------------------------------------------------------

#[derive(Template)]
#[template(path = "registry/remote_push.html")]
struct RemotePushPage {
    ctx: PageCtx,
    image_host: String,
    port: u16,
    /// Enabled Hostnames that target the Hosted Registry (M7); the push commands use the first.
    hostnames: Vec<String>,
    enabled: bool,
}

async fn remote_push(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let st = s
        .services
        .registry
        .status(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let hostnames = s
        .services
        .hostnames
        .list(&session.0.actor, None)
        .await
        .map_err(|e| DashError::new(e, &headers))?
        .into_iter()
        .filter(|h| h.target_kind == "registry" && h.enabled)
        .map(|h| h.hostname)
        .collect();
    Ok(render(&RemotePushPage {
        ctx: PageCtx::new(&session, "Registry: remote push", "/registry"),
        image_host: st.image_host,
        port: st.listen_port,
        hostnames,
        enabled: st.enabled,
    }))
}
