//! Dashboard adapter: server-rendered askama pages with htmx (docs/rewrite/03 b).
//!
//! Registering a feature module:
//! 1. create `dashboard/<area>.rs` with `pub fn routes() -> Router<AppState>`;
//! 2. add `mod <area>;` below and ONE `.merge(<area>::routes())` line in [`routes`]
//!    (in the authenticated block unless the page must be public);
//! 3. add its templates under `templates/<area>/`;
//! 4. add its nav entry to `crate::nav::NAV`.
//!
//! Everything in the authenticated block is behind [`session::require_session`]
//! (cookie auth + CSRF); a route can only become public by being added to
//! `auth::public_routes`.

mod account;
mod audit;
mod auth;
mod backups;
mod containers;
mod cron;
mod exec;
mod home;
mod session;
mod ssh;
mod system;
mod tokens;
mod users;
pub mod view;

pub use exec::ExecLimits;

use axum::Router;
use axum::middleware::from_fn_with_state;

use crate::app::AppState;

pub fn routes(state: &AppState) -> Router<AppState> {
    let authed = Router::new()
        // One line per feature module:
        .merge(home::routes())
        .merge(auth::session_routes())
        .merge(users::routes())
        .merge(tokens::routes())
        .merge(account::routes())
        .merge(audit::routes())
        .merge(containers::routes())
        .merge(system::routes())
        .merge(cron::routes())
        .merge(ssh::routes())
        .merge(backups::routes())
        .route_layer(from_fn_with_state(state.clone(), session::require_session));
    Router::new()
        .merge(auth::public_routes())
        .merge(authed)
        .layer(from_fn_with_state(state.clone(), session::origin_guard))
}
