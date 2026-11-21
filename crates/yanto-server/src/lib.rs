//! Dashboard, REST and MCP adapters. M0 ships the router skeleton only:
//! `/healthz`, the base layout at `/`, and embedded static assets.

mod app;
mod assets;
mod dashboard;

pub use app::{AppState, router, serve};
