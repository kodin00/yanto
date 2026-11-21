//! Webhook endpoints outside `/api/v1` and the dashboard session (03 a.6): `POST /deploy`
//! (bearer Deploy Token) and `POST /webhooks/github` (HMAC signature). They authenticate
//! themselves in the service layer, accept no cookie, and are not CSRF-protected.

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, FromRequestParts, Query, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::json;
use yanto_core::actor::Channel;
use yanto_core::service::deployments::{GithubDelivery, GithubOutcome};

use crate::app::AppState;
use crate::error::ApiError;
use crate::net::resolve_origin;

/// The socket peer, when the server was started with connect info.
struct Peer(Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0),
        ))
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/deploy", post(deploy))
        .route("/webhooks/github", post(github))
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// `202 {deployment, reused}`. The request body is ignored: no ref or env via a Deploy Token.
async fn deploy(
    State(s): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let origin = resolve_origin(&headers, peer.0, &s.trusted_proxies, Channel::DeployToken);
    let bearer = header(&headers, AUTHORIZATION.as_str())
        .and_then(|v| {
            let (scheme, rest) = v.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
        })
        .unwrap_or("");
    let id = q.get("id").map_or("", String::as_str);
    let out = s
        .services
        .deployments
        .deploy_webhook(&origin, id, bearer)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(out)).into_response())
}

/// GitHub push webhook. The body is capped at 1 MiB by the router's body limit (413).
async fn github(
    State(s): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let origin = resolve_origin(&headers, peer.0, &s.trusted_proxies, Channel::Webhook);
    let id = q.get("id").map_or("", String::as_str);
    let delivery = GithubDelivery {
        signature: header(&headers, "x-hub-signature-256"),
        event: header(&headers, "x-github-event"),
        delivery_id: header(&headers, "x-github-delivery"),
        form_encoded: header(&headers, CONTENT_TYPE.as_str())
            .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded")),
        body: &body,
    };
    Ok(
        match s
            .services
            .deployments
            .github_webhook(&origin, id, &delivery)
            .await?
        {
            GithubOutcome::Pong => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
            GithubOutcome::Duplicate => {
                (StatusCode::OK, Json(json!({ "status": "duplicate" }))).into_response()
            }
            GithubOutcome::Ignored(reason) => (
                StatusCode::ACCEPTED,
                Json(json!({ "status": "ignored", "reason": reason })),
            )
                .into_response(),
            GithubOutcome::Queued(out) => (StatusCode::ACCEPTED, Json(*out)).into_response(),
        },
    )
}
