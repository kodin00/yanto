//! `/backups`: local, downloadable Backups (`backup*`). Downloads need `admin`; the Yanto-DB
//! archive (it holds `app.key`) is Owner only.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use yanto_core::backup::{BackupKind, Trigger};
use yanto_core::service::backups::{BackupDto, BackupTarget};

use super::{ApiActor, ApiJson, created};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/backups", get(list).post(create))
        .route("/backups/targets", get(targets))
        .route("/backups/{id}", axum::routing::delete(delete))
        .route("/backups/{id}/download", get(download))
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
    let dto = if kind == BackupKind::YantoSqlite {
        s.services
            .backups
            .create_yanto(&a, Trigger::Api, None)
            .await?
    } else {
        let project = b.project_id.ok_or_else(|| {
            ApiError::from(yanto_core::CoreError::validation(
                "project_id",
                "project_id is required",
            ))
        })?;
        s.services
            .backups
            .create_project(&a, &project, kind, b.target.as_deref(), b.stop_project)
            .await?
    };
    Ok(created(dto))
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
