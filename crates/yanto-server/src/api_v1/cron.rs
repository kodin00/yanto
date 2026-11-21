//! `/cron`: Cron Entries (`cron_*`). Owner only; `admin` scope for create/update/delete.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yanto_core::cron::{EntryInput, Overlap};
use yanto_core::ports::CronSource;
use yanto_core::service::cron::{
    CronEntryDto, CronOverview, CronPatch, CronRunDto, SchedulePreview,
};

use super::{ApiActor, ApiJson, created};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/cron", get(list).post(create))
        .route("/cron/system-files", get(system_files))
        .route("/cron/preview", post(preview))
        .route("/cron/reapply", post(reapply))
        .route("/cron/{id}", axum::routing::patch(update).delete(delete))
        .route("/cron/{id}/runs", get(runs))
        .route("/cron/{id}/toggle", post(toggle))
}

fn overlap(s: &str) -> Result<Overlap, ApiError> {
    Overlap::parse(s).ok_or_else(|| {
        yanto_core::CoreError::validation("overlap", "overlap must be allow or skip").into()
    })
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    name: String,
    schedule: String,
    run_as_user: String,
    command: String,
    #[serde(default)]
    overlap: Option<String>,
    #[serde(default)]
    timeout_s: Option<i64>,
    #[serde(default = "yes")]
    enabled: bool,
}

fn yes() -> bool {
    true
}

async fn list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<CronOverview>, ApiError> {
    Ok(Json(s.services.cron.list(&a).await?))
}

async fn system_files(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<Vec<CronSource>>, ApiError> {
    Ok(Json(s.services.cron.files_list(&a).await?))
}

async fn runs(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<Vec<CronRunDto>>, ApiError> {
    Ok(Json(s.services.cron.runs_list(&a, &id).await?))
}

async fn create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<CreateBody>,
) -> Result<Response, ApiError> {
    let input = EntryInput {
        name: b.name,
        schedule: b.schedule,
        run_as_user: b.run_as_user,
        command: b.command,
        overlap: overlap(b.overlap.as_deref().unwrap_or("allow"))?,
        timeout_s: b.timeout_s,
        enabled: b.enabled,
    };
    Ok(created(s.services.cron.create(&a, &input).await?))
}

#[derive(Debug, Deserialize)]
struct PatchBody {
    name: Option<String>,
    schedule: Option<String>,
    run_as_user: Option<String>,
    command: Option<String>,
    overlap: Option<String>,
    /// Absent keeps the value; `null` is treated as absent too (send a number to change it).
    timeout_s: Option<i64>,
    enabled: Option<bool>,
}

async fn update(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<PatchBody>,
) -> Result<Json<CronEntryDto>, ApiError> {
    let patch = CronPatch {
        name: b.name,
        schedule: b.schedule,
        run_as_user: b.run_as_user,
        command: b.command,
        overlap: b.overlap.as_deref().map(overlap).transpose()?,
        timeout_s: b.timeout_s,
        enabled: b.enabled,
    };
    Ok(Json(s.services.cron.patch(&a, &id, &patch).await?))
}

#[derive(Debug, Deserialize)]
struct ToggleBody {
    enabled: bool,
}

async fn toggle(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<ToggleBody>,
) -> Result<Json<CronEntryDto>, ApiError> {
    Ok(Json(s.services.cron.toggle(&a, &id, b.enabled).await?))
}

async fn delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    s.services.cron.delete(&a, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Deserialize)]
struct PreviewBody {
    schedule: String,
    #[serde(default)]
    count: Option<usize>,
}

async fn preview(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<PreviewBody>,
) -> Result<Json<SchedulePreview>, ApiError> {
    Ok(Json(
        s.services
            .cron
            .preview(&a, &b.schedule, b.count.unwrap_or(5))
            .await?,
    ))
}

#[derive(Debug, Deserialize, Default)]
struct ReapplyBody {
    #[serde(default)]
    replace_unsafe: bool,
}

/// Overwrite `/etc/cron.d/yanto` with the stored entries (drift "re-apply").
async fn reapply(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    body: Option<Json<ReapplyBody>>,
) -> Result<Response, ApiError> {
    let replace = body.map(|b| b.0.replace_unsafe).unwrap_or_default();
    s.services.cron.reapply(&a, replace).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
