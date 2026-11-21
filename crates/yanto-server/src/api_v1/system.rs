//! `/api/v1`: `health`, `system_usage`, `system_cleanup_preview`, `system_cleanup`.

use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yanto_core::service::system::{
    CleanupPreview, CleanupResult, CleanupSelection, HealthReport, UsageReport,
};

use super::{ApiActor, ApiJson};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/system/usage", get(usage))
        .route("/system/logs", get(logs))
        .route("/system/cleanup/preview", get(preview))
        .route("/system/cleanup", post(cleanup))
}

async fn health(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<HealthReport>, ApiError> {
    Ok(Json(s.services.system.health(&a).await?))
}

async fn usage(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<UsageReport>, ApiError> {
    Ok(Json(s.services.system.usage(&a).await?))
}

async fn preview(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<CleanupPreview>, ApiError> {
    Ok(Json(s.services.system.cleanup_preview(&a).await?))
}

/// Omitted fields use the defaults of 04 3.1: build cache, dangling images and unused networks on,
/// stopped containers off (and, when on, `confirm` must be `"prune containers"`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupBody {
    build_cache: Option<bool>,
    images: Option<bool>,
    networks: Option<bool>,
    containers: Option<bool>,
    confirm: Option<String>,
}

async fn cleanup(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<CleanupBody>,
) -> Result<Json<CleanupResult>, ApiError> {
    let d = CleanupSelection::defaults();
    let sel = CleanupSelection {
        build_cache: b.build_cache.unwrap_or(d.build_cache),
        images: b.images.unwrap_or(d.images),
        networks: b.networks.unwrap_or(d.networks),
        containers: b.containers.unwrap_or(d.containers),
        confirm: b.confirm,
    };
    Ok(Json(s.services.system.cleanup(&a, &sel).await?))
}

#[derive(Debug, Deserialize)]
struct LogsParams {
    tail: Option<usize>,
}

/// Recent Yanto application log as text, `LEVEL target message` per line (Owner only).
async fn logs(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    params: Result<Query<LogsParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(p) = params.map_err(|_| ApiError::bad_request("invalid query string"))?;
    let lines = s.services.system.logs(&a, p.tail).await?;
    let body: String = lines
        .iter()
        .map(|l| format!("{} {} {}\n", l.level, l.target, l.text))
        .collect();
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response())
}
