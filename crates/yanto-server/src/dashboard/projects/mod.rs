//! Projects & Deployments dashboard (docs/rewrite/03 b): Projects, Deployments, Deploy Keys
//! and Registry Credentials pages. One `routes()` for the central registration line.

mod ansi;
mod deployments;
mod keys;
mod pages;

use axum::Router;

use crate::app::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(pages::routes())
        .merge(deployments::routes())
        .merge(keys::routes())
}

/// Redirect that works for plain forms (303) and htmx (`HX-Redirect`).
pub(super) fn redirect(headers: &axum::http::HeaderMap, to: &str) -> axum::response::Response {
    use axum::http::HeaderValue;
    use axum::response::IntoResponse;
    if super::view::is_htmx(headers) {
        let mut res = axum::http::StatusCode::NO_CONTENT.into_response();
        if let Ok(v) = HeaderValue::from_str(to) {
            res.headers_mut().insert("hx-redirect", v);
        }
        res
    } else {
        axum::response::Redirect::to(to).into_response()
    }
}

/// `<div class="toast-msg">..</div>` fragment (escaped).
pub(super) fn toast(msg: &str, error: bool) -> axum::response::Html<String> {
    axum::response::Html(format!(
        "<div class=\"toast-msg{}\">{}</div>",
        if error { " error" } else { "" },
        super::view::escape(msg)
    ))
}
