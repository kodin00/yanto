//! `/backups`: local, downloadable Backups (`backup*`). Downloads need `admin`; the Yanto-DB
//! archive (it holds `app.key`) is Owner only.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use yanto_core::backup::{BackupKind, Trigger};
use yanto_core::service::backups::{BackupDto, BackupTarget, RestoreOptions, RestoreOutcome};

use super::{ApiActor, ApiJson, created};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/backups", get(list).post(create))
        .route("/backups/targets", get(targets))
        .route("/backups/{id}", axum::routing::delete(delete))
        .route("/backups/{id}/download", get(download))
        .route("/backups/{id}/restore", axum::routing::post(restore))
}

async fn list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<Vec<BackupDto>>, ApiError> {
    Ok(Json(s.services.backups.list(&a).await?))
}

async fn targets(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<Vec<BackupTarget>>, ApiError> {
    Ok(Json(s.services.backups.targets(&a).await?))
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    kind: String,
    project_id: Option<String>,
    /// Declared database service or volume name for Project kinds.
    target: Option<String>,
    #[serde(default)]
    stop_project: bool,
}

async fn create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<CreateBody>,
) -> Result<Response, ApiError> {
    let kind = BackupKind::parse(&b.kind).ok_or_else(|| {
        ApiError::from(yanto_core::CoreError::validation(
            "kind",
            "kind must be yanto_sqlite, project_volumes, project_postgres or project_mysql",
        ))
    })?;
    if kind == BackupKind::YantoSqlite {
        let dto = s
            .services
            .backups
            .create_yanto(&a, Trigger::Api, None)
            .await?;
        return Ok(created(dto));
    }
    // Project kinds answer a list: a volume backup writes one archive per volume.
    let dtos = {
        let project = b.project_id.ok_or_else(|| {
            ApiError::from(yanto_core::CoreError::validation(
                "project_id",
                "project_id is required",
            ))
        })?;
        s.services
            .backups
            .create_project_with(
                &a,
                &project,
                kind,
                b.target.as_deref(),
                b.stop_project,
                Trigger::Api,
                None,
            )
            .await?
    };
    Ok(created(dtos))
}

#[derive(Debug, Default, Deserialize)]
struct RestoreBody {
    /// Empty the original volume instead of restoring into a new one.
    #[serde(default)]
    in_place: bool,
    /// The Project name, required for in-place volume and database restores.
    confirm_name: Option<String>,
}

async fn restore(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<RestoreBody>,
) -> Result<Json<RestoreOutcome>, ApiError> {
    Ok(Json(
        s.services
            .backups
            .restore(
                &a,
                &id,
                RestoreOptions {
                    in_place: b.in_place,
                    confirm_name: b.confirm_name,
                },
            )
            .await?,
    ))
}

async fn delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    s.services.backups.delete(&a, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn download(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let file = s.services.backups.download(&a, &id).await?;
    Ok(crate::download::file_response(file).await?)
}
