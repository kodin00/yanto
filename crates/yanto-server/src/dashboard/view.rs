//! Rendering helpers shared by every dashboard page.

use askama::Template;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use yanto_core::CoreError;

use super::session::Session;
use crate::error::{public_message, status_for};
use crate::nav;

#[derive(Debug, Clone)]
pub struct NavLink {
    pub label: &'static str,
    pub href: &'static str,
    pub active: bool,
}

/// Data the base layout needs on every page.
#[derive(Debug, Clone, Default)]
pub struct PageCtx {
    pub title: String,
    pub nav: Vec<NavLink>,
    /// Empty on public pages.
    pub username: String,
    /// Empty on public pages; rendered into `hx-headers` on `<body>`.
    pub csrf: String,
}

impl PageCtx {
    /// Context for a signed-in page at `path` (used to highlight the nav entry).
    pub fn new(session: &Session, title: impl Into<String>, path: &str) -> Self {
        let nav = nav::visible(&session.0.actor)
            .into_iter()
            .map(|e| NavLink {
                label: e.label,
                href: e.href,
                active: if e.href == "/" {
                    path == "/"
                } else {
                    path.starts_with(e.href)
                },
            })
            .collect();
        Self {
            title: title.into(),
            nav,
            username: session.0.actor.label.clone(),
            csrf: session.0.csrf_token.clone(),
        }
    }

    /// Context for pages shown before sign-in.
    pub fn public(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }
}

/// Render a template as `200 text/html`.
pub fn render<T: Template>(page: &T) -> Response {
    render_status(StatusCode::OK, page)
}

pub fn render_status<T: Template>(status: StatusCode, page: &T) -> Response {
    match page.render() {
        Ok(body) => (status, no_store_headers(), Html(body)).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Pages here can show one-time secrets (invite links, new tokens): never cache them.
pub fn no_store_headers() -> [(axum::http::HeaderName, HeaderValue); 1] {
    [(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    )]
}

pub fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("hx-request").is_some_and(|v| v == "true")
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    ctx: PageCtx,
    status: u16,
    message: String,
}

/// Error returned by dashboard handlers. Full-page requests get an error page; htmx requests
/// get a fragment retargeted to the `#toast` container (docs/rewrite/03 b).
#[derive(Debug)]
pub struct DashError {
    pub err: CoreError,
    pub htmx: bool,
}

impl DashError {
    pub fn new(err: CoreError, headers: &HeaderMap) -> Self {
        Self {
            err,
            htmx: is_htmx(headers),
        }
    }
}

impl IntoResponse for DashError {
    fn into_response(self) -> Response {
        let status = status_for(&self.err.kind);
        let message = public_message(&self.err);
        if self.htmx {
            let body = format!("<div class=\"toast-msg error\">{}</div>", escape(&message));
            let mut res = (status, Html(body)).into_response();
            let h = res.headers_mut();
            h.insert("hx-retarget", HeaderValue::from_static("#toast"));
            h.insert("hx-reswap", HeaderValue::from_static("innerHTML"));
            return res;
        }
        render_status(
            status,
            &ErrorPage {
                ctx: PageCtx::public("Error"),
                status: status.as_u16(),
                message,
            },
        )
    }
}

/// Minimal HTML escaping for the few fragments built by hand.
pub fn escape(s: &str) -> String {
    s.chars()
        .fold(String::with_capacity(s.len()), |mut out, c| {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                c => out.push(c),
            }
            out
        })
}

/// `2026-01-31 14:05 UTC` from unix milliseconds.
pub fn fmt_ts(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

pub fn fmt_opt_ts(ms: Option<i64>) -> String {
    ms.map_or_else(|| "never".to_owned(), fmt_ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_format() {
        assert_eq!(fmt_ts(0), "1970-01-01 00:00 UTC");
        assert_eq!(fmt_ts(1_767_225_600_000), "2026-01-01 00:00 UTC");
        assert_eq!(fmt_ts(1_709_210_096_000), "2024-02-29 12:34 UTC");
    }

    #[test]
    fn escaping() {
        assert_eq!(
            escape("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }
}
