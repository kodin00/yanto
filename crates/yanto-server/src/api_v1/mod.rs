//! REST `/api/v1` adapter: Bearer API Tokens only, JSON in and out.
//!
//! Cookies are never read here, so no CSRF applies (docs/rewrite/03 a.1). Handlers
//! parse input, call a service function with the token's [`Actor`] and serialize the
//! result; the service layer does all authorization.
//!
//! Registering a feature module: create `api_v1/<area>.rs` exposing
//! `pub fn routes() -> Router<AppState>` and add one `.merge(<area>::routes())`
//! line to [`routes`] below.

mod audit;
mod authorized_keys;
mod backups;
mod containers;
mod cron;
mod ingress;
mod me;
mod projects;
mod registry;
mod settings;
mod system;
mod users;

use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use std::net::SocketAddr;
use yanto_core::actor::{Actor, Channel};

use crate::app::AppState;
use crate::error::ApiError;
use crate::net::resolve_origin;

pub fn routes(state: &AppState) -> Router<AppState> {
    let _ = state;
    Router::new()
        .route("/", get(index))
        // One line per feature module:
        .merge(me::routes())
        .merge(audit::routes())
        .merge(users::routes())
        .merge(containers::routes())
        .merge(system::routes())
        .merge(cron::routes())
        .merge(authorized_keys::routes())
        .merge(backups::routes())
        .merge(projects::routes())
        .merge(registry::routes())
        .merge(settings::routes())
        .merge(ingress::routes())
        .fallback(not_found)
        .layer(axum::middleware::map_response(no_store))
}

async fn index() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "name": "yanto", "api": "v1" }))
}

async fn not_found() -> ApiError {
    ApiError {
        status: StatusCode::NOT_FOUND,
        code: "not_found",
        message: "no such endpoint".into(),
        fields: Vec::new(),
        retry_after: None,
    }
}

/// API responses can carry secrets (invite links); never cache them.
async fn no_store(mut res: Response) -> Response {
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// The [`Actor`] behind `Authorization: Bearer <API Token>`.
pub struct ApiActor(pub Actor);

impl FromRequestParts<AppState> for ApiActor {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0);
        let origin = resolve_origin(&parts.headers, peer, &state.trusted_proxies, Channel::Rest);
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                let (scheme, rest) = v.split_once(' ')?;
                scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
            })
            .unwrap_or("");
        Ok(Self(
            state.services.tokens.authenticate(&origin, token).await?,
        ))
    }
}

/// JSON body extractor whose rejection is the uniform error envelope.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(Self(v)),
            Err(rej) => Err(ApiError {
                status: rej.status(),
                code: if rej.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    "payload_too_large"
                } else {
                    "bad_request"
                },
                message: "invalid JSON request body".into(),
                fields: Vec::new(),
                retry_after: None,
            }),
        }
    }
}

pub fn created<T: serde::Serialize>(body: T) -> Response {
    (StatusCode::CREATED, Json(body)).into_response()
}
