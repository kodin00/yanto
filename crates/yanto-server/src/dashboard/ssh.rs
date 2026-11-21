//! `/ssh`: Authorized Keys for the configured Unix user (docs/rewrite/04 section 6). Owner only.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::service::authorized_keys::{AddKey, FileState, KeysOverview};

use super::session::Session;
use super::view::{DashError, PageCtx, fmt_ts, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/ssh", get(index).post(add))
        .route("/ssh/{id}", axum::routing::delete(remove))
}

struct KeyView {
    id: String,
    name: String,
    key_type: String,
    fingerprint: String,
    created: String,
}

#[derive(Template)]
#[template(path = "ssh/index.html")]
struct IndexPage {
    ctx: PageCtx,
    unix_user: String,
    problem: Option<String>,
    file_path: String,
    file_note: Option<String>,
    keys: Vec<KeyView>,
    outside_keys: usize,
    block_in_sync: bool,
    warnings: Vec<String>,
    markers_broken: bool,
    error: Option<String>,
    name: String,
    public_key: String,
}

fn page(
    session: &Session,
    o: KeysOverview,
    warnings_extra: Vec<String>,
    error: Option<String>,
    name: String,
    public_key: String,
) -> IndexPage {
    let (file_note, markers_broken) = match &o.file {
        FileState::Ambiguous { detail } => (Some(detail.clone()), true),
        FileState::Unreadable { detail } => (Some(detail.clone()), true),
        FileState::Missing => (
            Some("the file does not exist yet; it is created with the first key".into()),
            false,
        ),
        FileState::Ok => (None, false),
    };
    let mut warnings = o.warnings;
    warnings.extend(warnings_extra);
    IndexPage {
        ctx: PageCtx::new(session, "SSH keys", "/ssh"),
        unix_user: o.unix_user,
        problem: o.problem,
        file_path: o.file_path.unwrap_or_default(),
        file_note,
        keys: o
            .keys
            .into_iter()
            .map(|k| KeyView {
                id: k.id,
                name: k.name,
                key_type: k.key_type,
                fingerprint: k.fingerprint,
                created: fmt_ts(k.created_at),
            })
            .collect(),
        outside_keys: o.outside_keys,
        block_in_sync: o.block_in_sync,
        warnings,
        markers_broken,
        error,
        name,
        public_key,
    }
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let o = s
        .services
        .authorized_keys
        .list(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&page(
        &session,
        o,
        Vec::new(),
        None,
        String::new(),
        String::new(),
    )))
}

#[derive(Deserialize)]
struct AddForm {
    #[serde(default)]
    name: String,
    public_key: String,
}

async fn add(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<AddForm>,
) -> Result<Response, DashError> {
    let svc = &s.services.authorized_keys;
    let result = svc
        .add(
            &session.0.actor,
            &AddKey {
                name: f.name.clone(),
                public_key: f.public_key.clone(),
            },
        )
        .await;
    let reload = |extra: Vec<String>, error: Option<String>, name: String, key: String| async {
        let o = svc.list(&session.0.actor).await?;
        Ok::<_, CoreError>(page(&session, o, extra, error, name, key))
    };
    match result {
        Ok(added) => {
            let mut notes = added.warnings;
            if let Some(b) = added.backup_path {
                notes.push(format!("previous file saved as {b}"));
            }
            let p = reload(notes, None, String::new(), String::new())
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render(&p))
        }
        Err(e)
            if matches!(
                e.kind,
                yanto_core::ErrorKind::Validation(_) | yanto_core::ErrorKind::Conflict
            ) =>
        {
            let p = reload(Vec::new(), Some(public_message(&e)), f.name, f.public_key)
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(status_for(&e.kind), &p))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}

#[derive(Deserialize)]
struct RemoveParams {
    confirm_user: Option<String>,
}

/// htmx `hx-delete` on a key row. `confirm_user` (typed Unix user name) overrides the lockout guard.
async fn remove(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(p): Query<RemoveParams>,
) -> Result<Response, DashError> {
    s.services
        .authorized_keys
        .delete(
            &session.0.actor,
            &id,
            p.confirm_user.as_deref().filter(|c| !c.is_empty()),
        )
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    // Reload the page so the remaining count and the backup hint are current.
    let mut res = (StatusCode::OK, Html("")).into_response();
    res.headers_mut()
        .insert("hx-refresh", "true".parse().expect("static header"));
    Ok(res)
}
