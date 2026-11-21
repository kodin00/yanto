//! `/deploy-keys` and `/registry-credentials`: Owner-managed credentials. Private keys and
//! registry secrets are write-only; only public data is ever rendered.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::secrets::Secret;
use yanto_core::service::registry_credentials::CredentialUpdate;
use yanto_core::{CoreError, ErrorKind};

use super::redirect;
use crate::app::AppState;
use crate::dashboard::session::Session;
use crate::dashboard::view::{DashError, PageCtx, fmt_ts, render_status};
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/deploy-keys", get(keys_index).post(key_generate))
        .route("/deploy-keys/import", post(key_import))
        .route("/deploy-keys/{id}", delete(key_delete))
        .route("/registry-credentials", get(creds_index).post(cred_create))
        .route(
            "/registry-credentials/{id}",
            post(cred_update).delete(cred_delete),
        )
}

struct KeyView {
    id: String,
    name: String,
    key_type: String,
    public_key: String,
    fingerprint: String,
    created: String,
}

#[derive(Template)]
#[template(path = "projects/keys.html")]
struct KeysPage {
    ctx: PageCtx,
    keys: Vec<KeyView>,
    error: Option<String>,
}

async fn render_keys(
    s: &AppState,
    session: &Session,
    error: Option<String>,
    status: axum::http::StatusCode,
) -> Result<Response, CoreError> {
    let keys = s.services.deploy_keys.list(&session.0.actor).await?;
    Ok(render_status(
        status,
        &KeysPage {
            ctx: PageCtx::new(session, "Deploy keys", "/deploy-keys"),
            keys: keys
                .into_iter()
                .map(|k| KeyView {
                    created: fmt_ts(k.created_at),
                    id: k.id,
                    name: k.name,
                    key_type: k.key_type,
                    public_key: k.public_key,
                    fingerprint: k.fingerprint,
                })
                .collect(),
            error,
        },
    ))
}

async fn keys_index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    // Members cannot manage keys (Owner-only); the page itself is gated by the nav view and
    // by the service: listing is allowed, mutation forms fail with 403.
    render_keys(&s, &session, None, axum::http::StatusCode::OK)
        .await
        .map_err(|e| DashError::new(e, &headers))
}

#[derive(Deserialize)]
struct NameForm {
    name: String,
}

async fn form_result(
    s: &AppState,
    session: &Session,
    headers: &HeaderMap,
    r: Result<(), CoreError>,
    back: &str,
) -> Result<Response, DashError> {
    match r {
        Ok(()) => Ok(redirect(headers, back)),
        Err(e) if matches!(e.kind, ErrorKind::Validation(_) | ErrorKind::Conflict) => {
            let status = status_for(&e.kind);
            let res = if back == "/deploy-keys" {
                render_keys(s, session, Some(public_message(&e)), status).await
            } else {
                render_creds(s, session, Some(public_message(&e)), status).await
            };
            res.map_err(|e| DashError::new(e, headers))
        }
        Err(e) => Err(DashError::new(e, headers)),
    }
}

async fn key_generate(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<NameForm>,
) -> Result<Response, DashError> {
    let r = s
        .services
        .deploy_keys
        .create(&session.0.actor, &f.name)
        .await
        .map(|_| ());
    form_result(&s, &session, &headers, r, "/deploy-keys").await
}

#[derive(Deserialize)]
struct ImportForm {
    name: String,
    private_key: String,
}

async fn key_import(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<ImportForm>,
) -> Result<Response, DashError> {
    let r = s
        .services
        .deploy_keys
        .import(&session.0.actor, &f.name, &Secret::new(f.private_key))
        .await
        .map(|_| ());
    form_result(&s, &session, &headers, r, "/deploy-keys").await
}

async fn key_delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .deploy_keys
        .delete(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(redirect(&headers, "/deploy-keys"))
}

struct CredView {
    id: String,
    name: String,
    registry_host: String,
    username: String,
    updated: String,
}

#[derive(Template)]
#[template(path = "projects/credentials.html")]
struct CredsPage {
    ctx: PageCtx,
    creds: Vec<CredView>,
    error: Option<String>,
}

async fn render_creds(
    s: &AppState,
    session: &Session,
    error: Option<String>,
    status: axum::http::StatusCode,
) -> Result<Response, CoreError> {
    let creds = s
        .services
        .registry_credentials
        .list(&session.0.actor)
        .await?;
    Ok(render_status(
        status,
        &CredsPage {
            ctx: PageCtx::new(session, "Registry credentials", "/registry-credentials"),
            creds: creds
                .into_iter()
                .map(|c| CredView {
                    updated: fmt_ts(c.updated_at),
                    id: c.id,
                    name: c.name,
                    registry_host: c.registry_host,
                    username: c.username,
                })
                .collect(),
            error,
        },
    ))
}

async fn creds_index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    render_creds(&s, &session, None, axum::http::StatusCode::OK)
        .await
        .map_err(|e| DashError::new(e, &headers))
}

#[derive(Deserialize)]
struct CredForm {
    name: String,
    registry_host: String,
    username: String,
    secret: String,
}

async fn cred_create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<CredForm>,
) -> Result<Response, DashError> {
    let r = s
        .services
        .registry_credentials
        .create(
            &session.0.actor,
            &f.name,
            &f.registry_host,
            &f.username,
            &Secret::new(f.secret),
        )
        .await
        .map(|_| ());
    form_result(&s, &session, &headers, r, "/registry-credentials").await
}

#[derive(Deserialize, Default)]
struct CredUpdateForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    registry_host: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    secret: String,
}

/// Blank fields are left unchanged; a non-empty `secret` rotates it.
async fn cred_update(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<CredUpdateForm>,
) -> Result<Response, DashError> {
    let some = |v: &str| (!v.trim().is_empty()).then(|| v.trim().to_owned());
    let change = CredentialUpdate {
        name: some(&f.name),
        registry_host: some(&f.registry_host),
        username: some(&f.username),
        secret: (!f.secret.is_empty()).then(|| Secret::new(f.secret.clone())),
    };
    let r = s
        .services
        .registry_credentials
        .update(&session.0.actor, &id, &change)
        .await
        .map(|_| ());
    form_result(&s, &session, &headers, r, "/registry-credentials").await
}

async fn cred_delete(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .registry_credentials
        .delete(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(redirect(&headers, "/registry-credentials"))
}
