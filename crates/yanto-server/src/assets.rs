//! Static assets embedded in the binary.

use axum::Router;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rust_embed::RustEmbed;

use crate::app::AppState;

#[derive(RustEmbed)]
#[folder = "assets/"]
#[exclude = "*.md"]
struct Assets;

pub fn routes() -> Router<AppState> {
    Router::new().route("/assets/{*path}", get(asset))
}

async fn asset(Path(path): Path<String>, headers: HeaderMap) -> Response {
    let Some(file) = Assets::get(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let etag = format!("\"{}\"", hex(&file.metadata.sha256_hash()));
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == etag)
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        [
            (header::CONTENT_TYPE, file.metadata.mimetype().to_owned()),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, "public, max-age=3600".to_owned()),
        ],
        file.data.into_owned(),
    )
        .into_response()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
