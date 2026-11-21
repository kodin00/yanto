//! `/deployments`: queue and history, detail with live log (SSE), cancel, plain-text logs,
//! and the status event stream.

use std::convert::Infallible;

use askama::Template;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use tokio_stream::StreamExt;
use yanto_core::service::deployments::{
    self as dep, DeploymentDto, DeploymentQuery, LogStreamItem,
};

use super::ansi::ansi_to_html;
use super::redirect;
use crate::app::AppState;
use crate::dashboard::session::Session;
use crate::dashboard::view::{DashError, PageCtx, escape, fmt_opt_ts, fmt_ts, render};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/deployments", get(index))
        .route("/deployments/{id}", get(detail))
        .route("/deployments/{id}/cancel", post(cancel))
        .route("/projects/{id}/reset-checkout", post(reset_checkout))
        .route("/deployments/{id}/logs", get(logs_text))
        .route("/deployments/{id}/logs/stream", get(logs_stream))
        .route("/_f/deployments/active", get(active_fragment))
        .route("/events", get(events))
}

pub(super) struct DeploymentView {
    id: String,
    project_id: String,
    status: String,
    trigger: String,
    revision: String,
    queued: String,
    started: String,
    finished: String,
    error: String,
    triggered_by: String,
    active: bool,
}

impl From<&DeploymentDto> for DeploymentView {
    fn from(d: &DeploymentDto) -> Self {
        Self {
            id: d.id.clone(),
            project_id: d.project_id.clone(),
            status: d.status.clone(),
            trigger: d.trigger.clone(),
            revision: d
                .commit_sha
                .as_deref()
                .map(|c| {
                    let short: String = c.chars().take(8).collect();
                    format!("{short} {}", d.commit_message.as_deref().unwrap_or(""))
                })
                .or_else(|| d.image_digest.clone())
                .unwrap_or_default(),
            queued: fmt_ts(d.queued_at),
            started: fmt_opt_ts(d.started_at),
            finished: d.finished_at.map_or_else(|| "-".to_owned(), fmt_ts),
            error: d.error.clone().unwrap_or_default(),
            triggered_by: d.triggered_by.clone().unwrap_or_default(),
            active: matches!(d.status.as_str(), "queued" | "running"),
        }
    }
}

#[derive(Template)]
#[template(path = "deployments/index.html")]
struct IndexPage {
    ctx: PageCtx,
    active: Vec<DeploymentView>,
    recent: Vec<DeploymentView>,
}

async fn index(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let all = s
        .services
        .deployments
        .list(
            &session.0.actor,
            &DeploymentQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let (active, recent): (Vec<_>, Vec<_>) =
        all.iter().map(DeploymentView::from).partition(|d| d.active);
    Ok(render(&IndexPage {
        ctx: PageCtx::new(&session, "Deployments", "/deployments"),
        active,
        recent,
    }))
}

#[derive(Template)]
#[template(path = "deployments/detail.html")]
struct DetailPage {
    ctx: PageCtx,
    d: DeploymentView,
    can_cancel: bool,
    /// The pull failed to fast-forward and the actor may reset the checkout.
    can_reset: bool,
}

async fn detail(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let actor = &session.0.actor;
    let d = s
        .services
        .deployments
        .get(actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&DetailPage {
        ctx: PageCtx::new(&session, format!("Deployment {}", d.id), "/deployments"),
        can_cancel: actor
            .authorize(&dep::OP_DEPLOYMENT_CANCEL, Some(&d.project_id))
            .is_ok(),
        can_reset: d.reset_offered
            && actor
                .authorize(&dep::OP_PROJECT_RESET_CHECKOUT, Some(&d.project_id))
                .is_ok(),
        d: (&d).into(),
    }))
}

async fn cancel(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    s.services
        .deployments
        .cancel(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok(redirect(&headers, &format!("/deployments/{id}")))
}

async fn logs_text(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let text = s
        .services
        .deployments
        .logs(&session.0.actor, &id, None)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        text,
    )
        .into_response())
}

pub(crate) fn last_event_id(headers: &HeaderMap) -> Option<i64> {
    headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

/// SSE for the page: `log` events carry an escaped HTML fragment, `status`/`done` carry the status.
async fn logs_stream(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let stream = s
        .services
        .deployments
        .log_stream(&session.0.actor, &id, last_event_id(&headers))
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let events = stream.map(|item| {
        Ok::<_, Infallible>(match item {
            LogStreamItem::Log(c) => Event::default()
                .event("log")
                .id(c.seq.to_string())
                .data(ansi_to_html(&c.data)),
            LogStreamItem::Status(st) => Event::default().event("status").data(st),
            LogStreamItem::Done(st) => Event::default().event("done").data(st),
        })
    });
    let mut res = Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

async fn events(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let stream = s
        .services
        .deployments
        .events_stream(&session.0.actor)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let events = stream.map(|e| {
        Ok::<_, Infallible>(
            Event::default()
                .event(if e.done { "done" } else { "status" })
                .json_data(&e)
                .unwrap_or_else(|_| Event::default().event("status")),
        )
    });
    let mut res = Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

async fn active_fragment(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> Result<Response, DashError> {
    let all = s
        .services
        .deployments
        .list(
            &session.0.actor,
            &DeploymentQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let mut html = String::from("<ul>");
    for d in all
        .iter()
        .filter(|d| matches!(d.status.as_str(), "queued" | "running"))
    {
        html.push_str(&format!(
            "<li><a href=\"/deployments/{}\">{}</a> <span class=\"badge {}\">{}</span></li>",
            escape(&d.id),
            escape(&d.id),
            escape(&d.status),
            escape(&d.status)
        ));
    }
    html.push_str("</ul>");
    Ok(Html(html).into_response())
}

/// htmx: reset the Project's checkout to `origin/<branch>`, then answer a status fragment.
async fn reset_checkout(
    State(s): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, DashError> {
    let out = s
        .services
        .deployments
        .reset_checkout(&session.0.actor, &id)
        .await
        .map_err(|e| DashError::new(e, &headers))?;
    let short = |sha: &str| sha.chars().take(12).collect::<String>();
    Ok(super::toast(
        &format!(
            "Checkout reset to origin/{} ({}). Deploy again.",
            out.branch,
            short(&out.to)
        ),
        false,
    )
    .into_response())
}
