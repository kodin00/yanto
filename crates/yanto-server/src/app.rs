//! Router assembly and server loop.

use std::future::Future;
use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::routing::get;
use tokio::net::TcpListener;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use yanto_core::{Config, Db};

use crate::{assets, dashboard};

/// Default request body limit (1 MiB, as in the legacy server).
const BODY_LIMIT: usize = 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
}

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        Self {
            db,
            config: Arc::new(config),
        }
    }
}

pub fn router(state: AppState) -> Router {
    let header = |name: &'static str, value: &'static str| {
        SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        )
    };
    Router::new()
        .route("/healthz", get(healthz))
        .merge(dashboard::routes())
        .merge(assets::routes())
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(header(
            "content-security-policy",
            "default-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
        ))
        .layer(header("x-content-type-options", "nosniff"))
        .layer(header("referrer-policy", "same-origin"))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Unauthenticated liveness probe: `ok` when the database answers, else 503.
/// Deliberately minimal; no version or error detail.
async fn healthz(State(state): State<AppState>) -> (StatusCode, &'static str) {
    match state.db.ping().await {
        Ok(()) => (StatusCode::OK, "ok"),
        Err(err) => {
            tracing::error!(error = %err, source = ?err.source, "healthz: database check failed");
            (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        }
    }
}

/// Serve until `shutdown` resolves, then drain in-flight requests.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
}
