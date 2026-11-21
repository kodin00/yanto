//! Dashboard, REST and (later) MCP adapters over the `yanto-core` service layer.
//!
//! Adapters are thin: parse the request, build an `Actor`, call one service
//! function, render the result. They never check permissions themselves.

mod api_v1;
mod app;
mod assets;
mod dashboard;
mod download;
mod error;
pub mod mcp;
mod nav;
mod net;
mod stream_guard;
mod webhooks;

#[cfg(feature = "testing")]
pub mod testing;

pub use app::{AppState, router, serve};
pub use dashboard::ExecLimits;
