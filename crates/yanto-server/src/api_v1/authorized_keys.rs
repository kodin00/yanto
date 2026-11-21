//! `/authorized-keys`: Owner-managed SSH keys in the marked block (`authorized_keys_*`, `admin`).

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;
use yanto_core::service::authorized_keys::{AddKey, KeyAdded, KeyDeleted, KeysOverview};

use super::{ApiActor, ApiJson, created};
use crate::app::AppState;
use crate::error::ApiError;
use axum::response::Response;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/authorized-keys", get(list).post(add))
        .route("/authorized-keys/{id}", delete(remove))
}

async fn list(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
) -> Result<Json<KeysOverview>, ApiError> {
    Ok(Json(s.services.authorized_keys.list(&a).await?))
}

#[derive(Debug, Deserialize)]
struct AddBody {
    #[serde(default)]
    name: String,
    public_key: String,
}

async fn add(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    ApiJson(b): ApiJson<AddBody>,
) -> Result<Response, ApiError> {
    let out: KeyAdded = s
        .services
        .authorized_keys
        .add(
            &a,
            &AddKey {
                name: b.name,
                public_key: b.public_key,
            },
        )
        .await?;
    Ok(created(out))
}

#[derive(Debug, Deserialize)]
struct DeleteParams {
    /// The Unix user name, to override the lockout guard (audited as `authorized_key.force_change`).
    confirm_user: Option<String>,
}

async fn remove(
    State(s): State<AppState>,
    ApiActor(a): ApiActor,
    Path(id): Path<String>,
    params: Result<Query<DeleteParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<KeyDeleted>, ApiError> {
    let Query(p) = params.map_err(|_| ApiError::bad_request("invalid query string"))?;
    Ok(Json(
        s.services
            .authorized_keys
            .delete(&a, &id, p.confirm_user.as_deref())
            .await?,
    ))
}
