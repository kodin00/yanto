//! Session cookie, CSRF and same-origin middleware for the dashboard (docs/rewrite/03 a.3).
//!
//! Authentication is deny-by-default: every route registered in `dashboard::routes` under the
//! authenticated router passes through [`require_session`]. Only the explicitly public router
//! (login, setup, invite acceptance) skips it.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use yanto_core::CoreError;
use yanto_core::ErrorKind;
use yanto_core::actor::{Channel, Origin};
use yanto_core::crypto::ct_eq;
use yanto_core::service::auth::{SessionAuth, SessionGrant};

use super::view::{DashError, is_htmx};
use crate::app::AppState;
use crate::net::{resolve_origin, same_origin};

pub const COOKIE_NAME: &str = "yanto_session";
const FORM_LIMIT: usize = 1024 * 1024;
const COOKIE_MAX_AGE: i64 = 7 * 24 * 3600;

/// The authenticated session of the current request (inserted by [`require_session`]).
#[derive(Debug, Clone)]
pub struct Session(pub Arc<SessionAuth>);

impl<S: Send + Sync> FromRequestParts<S> for Session {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, StatusCode> {
        // Present whenever the route sits behind `require_session`; a handler that asks for it
        // on a public route is a programming error and fails closed.
        parts
            .extensions
            .get::<Session>()
            .cloned()
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

/// Where the request came from, for service calls made before/without a session.
pub struct Client(pub Origin);

impl FromRequestParts<AppState> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(origin_of(parts, state)))
    }
}

pub fn origin_of(parts: &Parts, state: &AppState) -> Origin {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    resolve_origin(
        &parts.headers,
        peer,
        &state.trusted_proxies,
        Channel::Dashboard,
    )
}

pub fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

pub(super) fn cookie_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| {
            let (k, v) = kv.trim().split_once('=')?;
            (k == COOKIE_NAME).then(|| v.to_owned())
        })
}

/// `Set-Cookie` value establishing a session (`HttpOnly; SameSite=Lax`, `Secure` over https).
pub fn set_cookie(state: &AppState, grant: &SessionGrant) -> HeaderValue {
    cookie_header(grant.cookie.expose(), COOKIE_MAX_AGE, state.cookie_secure())
}

pub fn clear_cookie(state: &AppState) -> HeaderValue {
    cookie_header("", 0, state.cookie_secure())
}

fn cookie_header(value: &str, max_age: i64, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}"
    ))
    .expect("cookie value is ascii")
}

fn is_unsafe(m: &Method) -> bool {
    !matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn forbidden(headers: &HeaderMap, msg: &str) -> Response {
    DashError::new(CoreError::forbidden(msg), headers).into_response()
}

/// Same-origin check for every state-changing dashboard request, public or not.
pub async fn origin_guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if is_unsafe(req.method()) && !same_origin(req.headers(), &state.config.server.public_url) {
        return forbidden(req.headers(), "cross-origin request blocked");
    }
    next.run(req).await
}

/// Authenticate the session cookie, enforce CSRF on unsafe methods and expose [`Session`].
pub async fn require_session(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let (mut parts, body) = req.into_parts();
    let origin = origin_of(&parts, &state);
    let Some(cookie) = cookie_value(&parts.headers) else {
        return unauthenticated(&state, &parts.headers).await;
    };
    let session = match state
        .services
        .auth
        .authenticate_session(&origin, &cookie)
        .await
    {
        Ok(s) => s,
        Err(e) if matches!(e.kind, ErrorKind::Unauthenticated) => {
            return unauthenticated(&state, &parts.headers).await;
        }
        Err(e) => return DashError::new(e, &parts.headers).into_response(),
    };

    let mut body = body;
    if is_unsafe(&parts.method) {
        // Synchronizer token: header (htmx) or `_csrf` form field (plain forms).
        let mut supplied = parts
            .headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        if supplied.is_none() && is_form(&parts.headers) {
            let Ok(bytes) = to_bytes(body, FORM_LIMIT).await else {
                return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response();
            };
            supplied = form_urlencoded::parse(&bytes)
                .find(|(k, _)| k == "_csrf")
                .map(|(_, v)| v.into_owned());
            body = Body::from(bytes);
        }
        if !supplied.is_some_and(|t| ct_eq(&t, &session.csrf_token)) {
            return forbidden(&parts.headers, "invalid or missing CSRF token");
        }
    }

    parts.extensions.insert(Session(Arc::new(session)));
    next.run(Request::from_parts(parts, body)).await
}

fn is_form(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"))
}

async fn unauthenticated(state: &AppState, headers: &HeaderMap) -> Response {
    let target = match state.services.auth.setup_pending().await {
        Ok(true) => "/setup",
        _ => "/login",
    };
    if is_htmx(headers) {
        let mut res = StatusCode::UNAUTHORIZED.into_response();
        res.headers_mut()
            .insert("hx-redirect", HeaderValue::from_static(target));
        return res;
    }
    Redirect::to(target).into_response()
}
