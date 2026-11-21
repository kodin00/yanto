//! `/deploy-keys`: public data only; the private half is write-only.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use yanto_core::db::repo::deploy_keys::DeployKeyPublic;
use yanto_core::secrets::Secret;

use crate::api_v1::{ApiActor, ApiJson, created};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/deploy-keys", get(list).post(create))
        .route("/deploy-keys/import", post(import))
        .route("/deploy-keys/{id}", get(get_one).delete(delete))
}

async fn list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<Vec<DeployKeyPublic>>, ApiError> {
    Ok(Json(s.services.deploy_keys.list(&a).await?))
}

async fn get_one(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<DeployKeyPublic>, ApiError> {
    Ok(Json(s.services.deploy_keys.public_get(&a, &id).await?))
}

#[derive(Debug, Deserialize)]
struct NameBody {
    name: String,
}

async fn create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<NameBody>,
) -> Result<Response, ApiError> {
    Ok(created(s.services.deploy_keys.create(&a, &b.name).await?))
}

#[derive(Debug, Deserialize)]
struct ImportBody {
    name: String,
    private_key: String,
}

async fn import(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<ImportBody>,
) -> Result<Response, ApiError> {
    Ok(created(
        s.services
            .deploy_keys
            .import(&a, &b.name, &Secret::new(b.private_key))
            .await?,
    ))
}

async fn delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    s.services.deploy_keys.delete(&a, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
