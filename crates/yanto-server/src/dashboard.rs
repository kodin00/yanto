//! Server-rendered dashboard pages (askama).

use askama::Template;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;

use crate::app::AppState;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    title: &'static str,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(index))
}

async fn index() -> Response {
    render(&IndexPage { title: "Yanto" })
}

fn render<T: Template>(page: &T) -> Response {
    match page.render() {
        Ok(body) => Html(body).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
