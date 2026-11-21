//! Streamable HTTP transport at `/mcp` (docs/rewrite/03 d).
//!
//! `Authorization: Bearer <API Token>` is resolved by the same `TokenService::authenticate`
//! as REST (20 failed attempts / 15 min / IP), with channel `mcp`; the resulting [`Actor`]
//! travels in the request extensions to the tool handlers. Cookies are never read.
//! Host and Origin are allow-listed (DNS-rebinding guard): loopback plus `server.public_url`.
//! The service is stateless (one JSON-RPC exchange per POST, no `Mcp-Session-Id`).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use yanto_core::actor::Channel;

use super::{McpServer, ToolSet};
use crate::app::AppState;
use crate::error::ApiError;
use crate::net::resolve_origin;

/// Router to `nest_service` at `/mcp`: Bearer middleware in front of the `rmcp` service.
pub fn router(state: &AppState) -> Router {
    let services = state.services.clone();
    let tools = Arc::new(ToolSet::standard());
    let svc = StreamableHttpService::new(
        move || Ok(McpServer::http(services.clone(), tools.clone())),
        Arc::new(NeverSessionManager::default()),
        config(&state.config.server.public_url),
    );
    Router::new()
        .fallback_service(svc)
        .layer(from_fn_with_state(state.clone(), bearer))
}

fn config(public_url: &str) -> StreamableHttpServerConfig {
    let mut hosts: Vec<String> = ["localhost", "127.0.0.1", "::1"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut origins: Vec<String> = ["http://localhost:*", "http://127.0.0.1:*"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if let Some(rest) = public_url.split_once("://").map(|(_, r)| r) {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if !authority.is_empty() {
            hosts.push(authority.to_owned());
            if let Some((host, _port)) = authority.rsplit_once(':')
                && !host.is_empty()
                && !host.ends_with(']')
            {
                hosts.push(host.to_owned());
            }
            origins.push(public_url.trim_end_matches('/').to_owned());
        }
    }
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(hosts)
        .with_allowed_origins(origins)
        .with_max_request_body_bytes(1024 * 1024)
}

async fn bearer(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let origin = resolve_origin(req.headers(), peer, &state.trusted_proxies, Channel::Mcp);
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            let (scheme, rest) = v.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
        })
        .unwrap_or("");
    match state.services.tokens.authenticate(&origin, token).await {
        Ok(actor) => {
            req.extensions_mut().insert(actor);
            next.run(req).await
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_allow_list_is_loopback_plus_public_url() {
        let c = config("");
        assert_eq!(c.allowed_hosts, ["localhost", "127.0.0.1", "::1"]);
        let c = config("https://yanto.example.com:8443/");
        assert!(
            c.allowed_hosts
                .contains(&"yanto.example.com:8443".to_owned())
        );
        assert!(c.allowed_hosts.contains(&"yanto.example.com".to_owned()));
        assert!(
            c.allowed_origins
                .contains(&"https://yanto.example.com:8443".to_owned())
        );
        assert!(!c.legacy_session_mode, "stateless");
    }
}
