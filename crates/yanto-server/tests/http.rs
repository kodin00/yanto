use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use tower::ServiceExt;
use yanto_core::{Config, Db};
use yanto_server::{AppState, router};

async fn app() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("yanto.db")).await.unwrap();
    (dir, AppState::new(db, Config::default()))
}

async fn get(state: &AppState, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let res = router(state.clone())
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (
        parts.status,
        parts.headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test]
async fn healthz_ok_when_db_is_up() {
    let (_dir, state) = app().await;
    let (status, _, body) = get(&state, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn healthz_503_when_db_is_closed() {
    let (_dir, state) = app().await;
    state.db.close().await;
    let (status, _, body) = get(&state, "/healthz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body, "unavailable");
}

#[tokio::test]
async fn index_renders_layout_with_security_headers() {
    let (_dir, state) = app().await;
    let (status, headers, body) = get(&state, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert!(body.contains("<title>Yanto</title>"));
    assert!(body.contains("/assets/vendor/htmx.min.js"));
    assert!(
        headers[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("default-src 'self'")
    );
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
}

#[tokio::test]
async fn embedded_htmx_is_served_with_etag_and_304() {
    let (_dir, state) = app().await;
    let (status, headers, body) = get(&state, "/assets/vendor/htmx.min.js").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("javascript")
    );
    assert!(body.contains("htmx"));

    let etag = headers[header::ETAG].clone();
    let res = router(state.clone())
        .oneshot(
            Request::get("/assets/vendor/htmx.min.js")
                .header(header::IF_NONE_MATCH, etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn missing_and_markdown_assets_are_404() {
    let (_dir, state) = app().await;
    assert_eq!(
        get(&state, "/assets/nope.js").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&state, "/assets/vendor/README.md").await.0,
        StatusCode::NOT_FOUND
    );
}
