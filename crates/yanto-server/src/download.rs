//! Streaming file download shared by the dashboard and REST backup routes (docs/rewrite/04 7.7).

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio_util::io::ReaderStream;
use yanto_core::CoreError;
use yanto_core::service::backups::DownloadFile;

/// `Content-Disposition: attachment` with `Content-Length` and `X-Content-SHA256`, streamed from
/// disk without loading the file. Backup names are `[0-9A-Za-z.-]`, so the quoted filename is safe.
pub async fn file_response(f: DownloadFile) -> Result<Response, CoreError> {
    let file = tokio::fs::File::open(&f.path)
        .await
        .map_err(|e| CoreError::not_found("backup file is missing").with_source(e))?;
    let safe: String = f
        .filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut res = (StatusCode::OK, Body::from_stream(ReaderStream::new(file))).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(f.size_bytes));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    if let Ok(v) = HeaderValue::from_str(&f.sha256) {
        h.insert("x-content-sha256", v);
    }
    Ok(res)
}
