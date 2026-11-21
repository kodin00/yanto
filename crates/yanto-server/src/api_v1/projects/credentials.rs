//! `/registry-credentials`: the secret is write-only.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, patch};
use axum::{Json, Router};
use serde::Deserialize;
use yanto_core::db::repo::registry_credentials::CredentialPublic;
use yanto_core::secrets::Secret;
use yanto_core::service::registry_credentials::CredentialUpdate;

use crate::api_v1::{ApiActor, ApiJson, created};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/registry-credentials", get(list).post(create))
        .route("/registry-credentials/{id}", patch(update).delete(delete))
}

async fn list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<Vec<CredentialPublic>>, ApiError> {
    Ok(Json(s.services.registry_credentials.list(&a).await?))
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    name: String,
    registry_host: String,
    username: String,
    secret: String,
}

async fn create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<CreateBody>,
) -> Result<Response, ApiError> {
    Ok(created(
        s.services
            .registry_credentials
            .create(
                &a,
                &b.name,
                &b.registry_host,
                &b.username,
                &Secret::new(b.secret),
            )
            .await?,
    ))
}

#[derive(Debug, Deserialize, Default)]
struct UpdateBody {
    name: Option<String>,
    registry_host: Option<String>,
    username: Option<String>,
    secret: Option<String>,
}

async fn update(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<UpdateBody>,
) -> Result<Json<CredentialPublic>, ApiError> {
    let change = CredentialUpdate {
        name: b.name,
        registry_host: b.registry_host,
        username: b.username,
        secret: b.secret.map(Secret::new),
    };
    Ok(Json(
        s.services
            .registry_credentials
            .update(&a, &id, &change)
            .await?,
    ))
}

async fn delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    s.services.registry_credentials.delete(&a, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
