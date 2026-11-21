//! `/projects/{id}/deploy|rollback*` and `/deployments*`.

use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tokio_stream::StreamExt;
use yanto_core::secrets::Secret;
use yanto_core::service::deployments::{
    DeployRequest, DeploymentDto, DeploymentQuery, LogStreamItem, RollbackPreview, RollbackRequest,
};

use crate::api_v1::ApiActor;
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/projects/{id}/deploy", post(deploy))
        .route("/projects/{id}/rollback/preview", post(rollback_preview))
        .route("/projects/{id}/rollback", post(rollback))
        .route("/projects/{id}/reset-checkout", post(reset_checkout))
        .route("/deployments", get(list))
        .route("/deployments/{id}", get(get_one))
        .route("/deployments/{id}/logs", get(logs))
        .route("/deployments/{id}/logs/stream", get(logs_stream))
        .route("/deployments/{id}/cancel", post(cancel))
}

#[derive(Debug, Deserialize, Default)]
struct DeployBody {
    target_ref: Option<String>,
    /// `.env` text carried by this deployment (also needs the `secrets` permission).
    env: Option<String>,
}

async fn deploy(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let b: DeployBody = super::optional_body(&body)?;
    let out = s
        .services
        .deployments
        .deploy(
            &a,
            &id,
            &DeployRequest {
                target_ref: b.target_ref,
                env: b.env.map(Secret::new),
            },
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(out)).into_response())
}

#[derive(Debug, Deserialize, Default)]
struct RollbackBody {
    deployment_id: Option<String>,
    git_ref: Option<String>,
}

impl From<RollbackBody> for RollbackRequest {
    fn from(b: RollbackBody) -> Self {
        Self {
            deployment_id: b.deployment_id,
            git_ref: b.git_ref,
        }
    }
}

async fn rollback_preview(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<RollbackPreview>, ApiError> {
    let req: RollbackRequest = super::optional_body::<RollbackBody>(&body)?.into();
    Ok(Json(
        s.services
            .deployments
            .rollback_preview(&a, &id, &req)
            .await?,
    ))
}

async fn rollback(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let req: RollbackRequest = super::optional_body::<RollbackBody>(&body)?.into();
    let out = s.services.deployments.rollback(&a, &id, &req).await?;
    Ok((StatusCode::ACCEPTED, Json(out)).into_response())
}

/// `git reset --hard origin/<branch>` after a non-fast-forward pull (audited, admin scope).
async fn reset_checkout(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    Ok(Json(s.services.deployments.reset_checkout(&a, &id).await?).into_response())
}

#[derive(Debug, Deserialize, Default)]
struct ListQuery {
    project_id: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
}

async fn list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<DeploymentDto>>, ApiError> {
    Ok(Json(
        s.services
            .deployments
            .list(
                &a,
                &DeploymentQuery {
                    project_id: q.project_id,
                    status: q.status,
                    limit: q.limit,
                },
            )
            .await?,
    ))
}

async fn get_one(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<DeploymentDto>, ApiError> {
    Ok(Json(s.services.deployments.get(&a, &id).await?))
}

#[derive(Debug, Deserialize, Default)]
struct TailQuery {
    tail: Option<usize>,
}

async fn logs(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    Query(q): Query<TailQuery>,
) -> Result<Response, ApiError> {
    let text = s.services.deployments.logs(&a, &id, q.tail).await?;
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response())
}

/// SSE: `event: log` (`id: <seq>`, JSON chunk), `event: status`, `event: done`.
async fn logs_stream(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let last = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let stream = s.services.deployments.log_stream(&a, &id, last).await?;
    let events = stream.map(|item| {
        Ok::<_, Infallible>(match item {
            LogStreamItem::Log(c) => Event::default()
                .event("log")
                .id(c.seq.to_string())
                .json_data(&c)
                .unwrap_or_else(|_| Event::default().event("log")),
            LogStreamItem::Status(st) => Event::default().event("status").data(st),
            LogStreamItem::Done(st) => Event::default().event("done").data(st),
        })
    });
    Ok(Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response())
}

async fn cancel(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<DeploymentDto>, ApiError> {
    Ok(Json(s.services.deployments.cancel(&a, &id).await?))
}
