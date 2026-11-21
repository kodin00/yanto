//! `/`: landing page. Later milestones add usage and recent Deployments here.

use askama::Template;
use axum::Router;
use axum::response::Response;
use axum::routing::get;

use super::session::Session;
use super::view::{PageCtx, render};
use crate::app::AppState;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    ctx: PageCtx,
    role: &'static str,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(index))
}

async fn index(session: Session) -> Response {
    render(&IndexPage {
        ctx: PageCtx::new(&session, "Dashboard", "/"),
        role: session.0.actor.role.as_str(),
    })
}
