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
use yanto_core::service::Services;
use yanto_core::{Config, Db};

use crate::net::{Cidr, default_trusted_proxies};
use crate::{api_v1, assets, dashboard, webhooks};

/// Default request body limit (1 MiB, as in the legacy server).
const BODY_LIMIT: usize = 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub services: Arc<Services>,
    /// Peers whose `X-Forwarded-For` is honored (`runtime.trusted_proxies`).
    pub trusted_proxies: Arc<Vec<Cidr>>,
    /// Terminal session timeouts (04 2.2); tests shorten them.
    pub exec_limits: crate::dashboard::ExecLimits,
}

impl AppState {
    pub fn new(db: Db, config: Config, services: Services) -> Self {
        Self {
            db,
            config: Arc::new(config),
            services: Arc::new(services),
            trusted_proxies: Arc::new(default_trusted_proxies()),
            exec_limits: crate::dashboard::ExecLimits::default(),
        }
    }

    /// Like [`AppState::new`], reading `runtime.trusted_proxies` from `app_settings`.
    pub async fn load(db: Db, config: Config, services: Services) -> Self {
        let mut state = Self::new(db, config, services);
        if let Some(list) = state.configured_trusted_proxies().await {
            state.trusted_proxies = Arc::new(list);
        }
        state
    }

    async fn configured_trusted_proxies(&self) -> Option<Vec<Cidr>> {
        let mut conn = self.db.reader().acquire().await.ok()?;
        let raw = yanto_core::db::repo::settings::get(&mut conn, "runtime.trusted_proxies")
            .await
            .ok()??;
        let list: Vec<String> = serde_json::from_str(&raw).ok()?;
        Some(crate::net::parse_cidrs(list.iter().map(String::as_str)))
    }

    /// Cookie `Secure` flag follows `server.public_url` (docs/rewrite/03 a.3).
    pub fn cookie_secure(&self) -> bool {
        self.config.server.public_url.starts_with("https://")
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
        .merge(assets::routes())
        .merge(webhooks::routes())
        .nest("/api/v1", api_v1::routes(&state))
        .merge(dashboard::routes(&state))
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
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}
