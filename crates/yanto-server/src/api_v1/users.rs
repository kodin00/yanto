//! `/users`: Owner-managed Members (`user_*`, `admin` scope).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use yanto_core::actor::Permission;
use yanto_core::db::repo::users::UserStatus;
use yanto_core::service::users::{GrantInput, UserDto};

use super::{ApiActor, ApiJson, created};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users", get(list).post(create))
        .route("/users/{id}", get(get_one).delete(delete))
        .route("/users/{id}/access", put(update_access))
        .route("/users/{id}/status", patch(set_status))
        .route("/users/{id}/reset-link", post(reset_link))
}

#[derive(Debug, Deserialize)]
struct GrantBody {
    project_id: String,
    #[serde(default)]
    permissions: Vec<String>,
}

fn grants(body: Vec<GrantBody>) -> Result<Vec<GrantInput>, ApiError> {
    body.into_iter()
        .map(|g| {
            let permissions = g
                .permissions
                .iter()
                .map(|p| p.parse::<Permission>())
                .collect::<Result<Vec<_>, _>>()?;
            Ok(GrantInput {
                project_id: g.project_id,
                permissions,
            })
        })
        .collect()
}

async fn list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    Ok(Json(s.services.users.list(&a).await?))
}

async fn get_one(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<UserDto>, ApiError> {
    Ok(Json(s.services.users.get(&a, &id).await?))
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    username: String,
    #[serde(default)]
    grants: Vec<GrantBody>,
}

/// Reveals the invite URL once.
async fn create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(body): ApiJson<CreateBody>,
) -> Result<Response, ApiError> {
    let out = s
        .services
        .users
        .create(&a, &body.username, &grants(body.grants)?)
        .await?;
    Ok(created(json!({
        "user": out.user,
        "invite_url": out.invite.url.expose(),
        "invite_expires_at": out.invite.expires_at,
    })))
}

#[derive(Debug, Deserialize)]
struct AccessBody {
    grants: Vec<GrantBody>,
}

async fn update_access(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<AccessBody>,
) -> Result<Json<UserDto>, ApiError> {
    Ok(Json(
        s.services
            .users
            .update_access(&a, &id, &grants(body.grants)?)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct StatusBody {
    status: String,
}

async fn set_status(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<StatusBody>,
) -> Result<Json<UserDto>, ApiError> {
    let status = UserStatus::parse(&body.status).ok_or_else(|| {
        ApiError::from(yanto_core::CoreError::validation(
            "status",
            "status must be active or disabled",
        ))
    })?;
    Ok(Json(s.services.users.set_status(&a, &id, status).await?))
}

/// Reveals the reset URL once; kills the user's sessions.
async fn reset_link(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let link = s.services.users.reset_link(&a, &id).await?;
    Ok(Json(
        json!({ "url": link.url.expose(), "expires_at": link.expires_at }),
    ))
}

async fn delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    s.services.users.delete(&a, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
