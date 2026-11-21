//! Router-level tests: health, layout, security headers, embedded assets.

use axum::http::{StatusCode, header};
use yanto_server::testing::TestApp;

#[tokio::test]
async fn healthz_ok_when_db_is_up() {
    let app = TestApp::new().await;
    let res = app.client().get("/healthz").await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.text(), "ok");
}

#[tokio::test]
async fn healthz_503_when_db_is_closed() {
    let app = TestApp::new().await;
    app.state.db.close().await;
    let res = app.client().get("/healthz").await;
    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.text(), "unavailable");
}

#[tokio::test]
async fn dashboard_renders_layout_with_nav_and_security_headers() {
    let app = TestApp::new().await;
    let res = app.owner_client().await.get("/").await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.header("content-type").unwrap().starts_with("text/html"));
    let body = res.text();
    assert!(body.contains("<title>Dashboard</title>"));
    assert!(body.contains("/assets/vendor/htmx.min.js"));
    assert!(
        body.contains("href=\"/users\""),
        "Owner sees Users in the nav"
    );
    assert!(body.contains("action=\"/logout\""));
    assert!(
        res.header("content-security-policy")
            .unwrap()
            .contains("default-src 'self'")
    );
    assert_eq!(res.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(res.header("cache-control"), Some("no-store"));
}

#[tokio::test]
async fn embedded_htmx_is_served_with_etag_and_304() {
    let app = TestApp::new().await;
    let c = app.client();
    let res = c.get("/assets/vendor/htmx.min.js").await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.header("content-type").unwrap().contains("javascript"));
    assert!(res.text().contains("htmx"));
    let etag = res.header("etag").unwrap().to_owned();
    let again = c
        .send(
            axum::http::Method::GET,
            "/assets/vendor/htmx.min.js",
            None,
            &[(header::IF_NONE_MATCH.as_str(), &etag)],
        )
        .await;
    assert_eq!(again.status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn missing_and_markdown_assets_are_404() {
    let app = TestApp::new().await;
    let c = app.client();
    assert_eq!(c.get("/assets/nope.js").await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        c.get("/assets/vendor/README.md").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn account_setup_script_is_embedded() {
    let app = TestApp::new().await;
    let res = app.client().get("/assets/js/account-setup.js").await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.text().contains("token"));
}
