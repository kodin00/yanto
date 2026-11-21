//! `/settings/api-tokens`: create, list and revoke API Tokens (dashboard only, D29).
//! Members manage their own tokens; the Owner sees and can revoke all of them.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Form, Router};
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::actor::Scope;
use yanto_core::service::tokens::TokenDto;

use super::session::Session;
use super::view::{DashError, PageCtx, fmt_opt_ts, fmt_ts, render, render_status};
use crate::app::AppState;
use crate::error::{public_message, status_for};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/api-tokens", get(index).post(create))
        .route("/settings/api-tokens/{id}", delete(revoke))
}

struct TokenView {
    id: String,
    name: String,
    hint: String,
    scope: String,
    owner: String,
    expires: String,
    last_used: String,
    created: String,
    revoked: bool,
}

impl From<TokenDto> for TokenView {
    fn from(t: TokenDto) -> Self {
        Self {
            expires: t.expires_at.map_or_else(|| "never".into(), fmt_ts),
            last_used: fmt_opt_ts(t.last_used_at),
            created: fmt_ts(t.created_at),
            revoked: t.revoked_at.is_some(),
            id: t.id,
            name: t.name,
            hint: t.hint,
            scope: t.scope,
            owner: t.username,
        }
    }
}

#[derive(Template)]
#[template(path = "tokens.html")]
struct TokensPage {
    ctx: PageCtx,
    tokens: Vec<TokenView>,
    show_owner: bool,
    /// Full token, shown exactly once right after creation.
    created_token: Option<String>,
    error: Option<String>,
    name: String,
}

async fn page(
    s: &AppState,
    session: &Session,
    created_token: Option<String>,
    error: Option<String>,
    name: String,
) -> Result<TokensPage, CoreError> {
    let tokens = s.services.tokens.list(&session.0.actor).await?;
    Ok(TokensPage {
        ctx: PageCtx::new(session, "API tokens", "/settings/api-tokens"),
        show_owner: session.0.actor.is_owner(),
        tokens: tokens.into_iter().map(Into::into).collect(),
        created_token,
        error,
        name,
    })
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let p = page(&s, &session, None, None, String::new())
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&p))
}

#[derive(Deserialize)]
struct CreateForm {
    name: String,
    scope: String,
    #[serde(default)]
    expires_in_days: String,
}

async fn create(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(f): Form<CreateForm>,
) -> Result<Response, DashError> {
    let result = async {
        let scope: Scope = f.scope.parse()?;
        let days = match f.expires_in_days.trim() {
            "" => None,
            d => Some(d.parse::<u32>().map_err(|_| {
                CoreError::validation("expires_in_days", "expiry must be a number of days")
            })?),
        };
        s.services
            .tokens
            .create(&session.0.actor, &f.name, scope, days)
            .await
    }
    .await;
    match result {
        Ok(created) => {
            let p = page(
                &s,
                &session,
                Some(created.token.expose().to_owned()),
                None,
                String::new(),
            )
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
            let p = page(&s, &session, None, Some(public_message(&e)), f.name)
                .await
                .map_err(|e| DashError::new(e, &headers))?;
            Ok(render_status(status_for(&e.kind), &p))
        }
        Err(e) => Err(DashError::new(e, &headers)),
    }
}

/// htmx `hx-delete` on a table row: an empty 200 removes the row.
async fn revoke(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .tokens
        .revoke(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok((StatusCode::OK, Html("")).into_response())
}
