//! `/registry`: the Hosted Registry (`registry_*`). Owner only. A generated registry password is
//! returned exactly once, in the response that created or reset it.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use yanto_core::secrets::Secret;
use yanto_core::service::registry::{
    EnableInput, GcPreview, GcStatus, RegistryStatus, RepoPage, TagDeletePreview, TagPage,
};

use super::{ApiActor, ApiJson, created};
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/registry", get(status).patch(settings))
        .route("/registry/enable", post(enable))
        .route("/registry/disable", post(disable))
        .route("/registry/restart", post(restart))
        .route("/registry/rerender", post(rerender))
        .route("/registry/users", get(users).post(user_create))
        .route("/registry/users/{id}", delete(user_delete))
        .route("/registry/users/{id}/password", put(user_password))
        .route("/registry/repos", get(repos))
        .route("/registry/repos/{name}/tags", get(tags))
        .route(
            "/registry/repos/{name}/tags/{tag}",
            get(tag_preview).delete(tag_delete),
        )
        .route("/registry/gc", get(gc_status).post(gc_start))
        .route("/registry/gc/preview", post(gc_preview))
}

async fn status(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<RegistryStatus>, ApiError> {
    Ok(Json(s.services.registry.status(&a).await?))
}

#[derive(Debug, Deserialize)]
struct SettingsBody {
    public_host: Option<String>,
}

async fn settings(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<SettingsBody>,
) -> Result<Json<RegistryStatus>, ApiError> {
    Ok(Json(
        s.services
            .registry
            .settings_update(&a, b.public_host.as_deref())
            .await?,
    ))
}

#[derive(Debug, Default, Deserialize)]
struct EnableBody {
    listen_port: Option<u16>,
}

async fn enable(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    body: Bytes,
) -> Result<Json<RegistryStatus>, ApiError> {
    let b: EnableBody = optional_json(&body)?;
    Ok(Json(
        s.services
            .registry
            .enable(
                &a,
                &EnableInput {
                    listen_port: b.listen_port,
                },
            )
            .await?,
    ))
}

#[derive(Debug, Default, Deserialize)]
struct DisableBody {
    /// The phrase `delete registry data` removes the stored images too.
    delete_data: Option<String>,
}

async fn disable(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    body: Bytes,
) -> Result<Json<RegistryStatus>, ApiError> {
    let b: DisableBody = optional_json(&body)?;
    Ok(Json(
        s.services
            .registry
            .disable(&a, b.delete_data.as_deref())
            .await?,
    ))
}

async fn restart(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<RegistryStatus>, ApiError> {
    Ok(Json(s.services.registry.restart(&a).await?))
}

async fn rerender(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<RegistryStatus>, ApiError> {
    Ok(Json(s.services.registry.rerender(&a).await?))
}

async fn users(State(s): State<AppState>, ApiActor(a): ApiActor) -> Result<Response, ApiError> {
    let users = s.services.registry.users_list(&a).await?;
    Ok(Json(json!({ "users": users })).into_response())
}

#[derive(Debug, Deserialize)]
struct UserBody {
    username: String,
    /// Omit to have Yanto generate one (returned once).
    password: Option<String>,
}

async fn user_create(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<UserBody>,
) -> Result<Response, ApiError> {
    let pw = b.password.map(Secret::new);
    let out = s
        .services
        .registry
        .user_create(&a, &b.username, pw.as_ref())
        .await?;
    Ok(created(json!({
        "user": out.user,
        "password": out.generated_password.as_ref().map(Secret::expose),
    })))
}

#[derive(Debug, Default, Deserialize)]
struct PasswordBody {
    password: Option<String>,
}

async fn user_password(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let pw = optional_json::<PasswordBody>(&body)?
        .password
        .map(Secret::new);
    let generated = s
        .services
        .registry
        .user_password_set(&a, &id, pw.as_ref())
        .await?;
    Ok(Json(
        json!({ "password": generated.as_ref().map(Secret::expose) }),
    ))
}

async fn user_delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    s.services.registry.user_delete(&a, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct PageQuery {
    last: Option<String>,
    n: Option<u32>,
}

type Q<T> = Result<Query<T>, axum::extract::rejection::QueryRejection>;

async fn repos(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    q: Q<PageQuery>,
) -> Result<Json<RepoPage>, ApiError> {
    let Query(q) = q.map_err(|_| ApiError::bad_request("invalid query string"))?;
    Ok(Json(
        s.services
            .registry
            .repos_list(&a, q.last.as_deref(), q.n)
            .await?,
    ))
}

async fn tags(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(name): Path<String>,
    q: Q<PageQuery>,
) -> Result<Json<TagPage>, ApiError> {
    let Query(q) = q.map_err(|_| ApiError::bad_request("invalid query string"))?;
    Ok(Json(
        s.services
            .registry
            .tags_list(&a, &name, q.last.as_deref(), q.n)
            .await?,
    ))
}

/// What deleting the tag would remove (the digest and its sibling tags). Deletes nothing.
async fn tag_preview(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path((name, tag)): Path<(String, String)>,
) -> Result<Json<TagDeletePreview>, ApiError> {
    Ok(Json(
        s.services
            .registry
            .tag_delete_preview(&a, &name, &tag)
            .await?,
    ))
}

async fn tag_delete(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path((name, tag)): Path<(String, String)>,
) -> Result<Json<TagDeletePreview>, ApiError> {
    Ok(Json(s.services.registry.tag_delete(&a, &name, &tag).await?))
}

#[derive(Debug, Default, Deserialize)]
struct GcBody {
    #[serde(default)]
    delete_untagged: bool,
}

async fn gc_status(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<GcStatus>, ApiError> {
    Ok(Json(s.services.registry.gc_status(&a).await?))
}

/// Starts the collection in the background (202); poll `GET /registry/gc`.
async fn gc_start(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    body: Bytes,
) -> Result<Response, ApiError> {
    let b: GcBody = optional_json(&body)?;
    let st = s.services.registry.gc_start(&a, b.delete_untagged).await?;
    Ok((StatusCode::ACCEPTED, Json(st)).into_response())
}

async fn gc_preview(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    body: Bytes,
) -> Result<Json<GcPreview>, ApiError> {
    let b: GcBody = optional_json(&body)?;
    Ok(Json(
        s.services
            .registry
            .gc_preview(&a, b.delete_untagged)
            .await?,
    ))
}

/// An optional JSON body: empty means "all defaults" (an action with no arguments).
fn optional_json<T: DeserializeOwned + Default>(body: &Bytes) -> Result<T, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|_| ApiError::bad_request("invalid JSON request body"))
}
