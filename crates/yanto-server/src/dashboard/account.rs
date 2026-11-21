//! `/settings/sessions` and `/settings/password`: the signed-in user's own sessions and password.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::CoreError;

use super::session::Session;
use super::view::{DashError, PageCtx, fmt_ts, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/sessions", get(index))
        .route("/settings/sessions/{id}", delete(revoke))
        .route("/settings/password", post(change_password))
}

struct SessionView {
    id: String,
    created: String,
    last_seen: String,
    ip: String,
    user_agent: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "account.html")]
struct AccountPage {
    ctx: PageCtx,
    sessions: Vec<SessionView>,
    message: Option<String>,
    error: Option<String>,
}

async fn page(
    s: &AppState,
    session: &Session,
    message: Option<String>,
    error: Option<String>,
) -> Result<AccountPage, CoreError> {
    let list = s
        .services
        .auth
        .session_list(&session.0.actor, &session.0.session_id)
        .await?;
    Ok(AccountPage {
        ctx: PageCtx::new(session, "Account", "/settings/sessions"),
        sessions: list
            .into_iter()
            .map(|x| SessionView {
                created: fmt_ts(x.created_at),
                last_seen: fmt_ts(x.last_seen_at),
                ip: x.ip.unwrap_or_default(),
                user_agent: x.user_agent.unwrap_or_default(),
                current: x.current,
                id: x.id,
            })
            .collect(),
        message,
        error,
    })
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    Ok(render(
        &page(&s, &session, None, None)
            .await
            .map_err(|e| DashError::new(e, &headers))?,
    ))
}

/// htmx `hx-delete` on a table row.
async fn revoke(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .auth
        .session_revoke(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok((StatusCode::OK, Html("")).into_response())
}

#[derive(Deserialize)]
struct PasswordForm {
    current_password: String,
    new_password: String,
    confirm: String,
}

async fn change_password(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<PasswordForm>,
) -> Result<Response, DashError> {
    let result = if f.new_password == f.confirm {
        s.services
            .auth
            .password_change(
                &session.0.actor,
                &session.0.session_id,
                &f.current_password,
                &f.new_password,
            )
            .await
    } else {
        Err(CoreError::validation("confirm", "Passwords do not match"))
    };
    match result {
        Ok(()) => {
            let p = page(
                &s,
                &session,
                Some("Password changed. Other sessions were signed out.".into()),
                None,
            )
            .await
            .map_err(|e| DashError::new(e, &headers))?;
            Ok(render(&p))
        }
        Err(e) if matches!(e.kind, yanto_core::ErrorKind::Validation(_)) => {
            let p = page(&s, &session, None, Some(public_message(&e)))
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(status_for(&e.kind), &p))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}
