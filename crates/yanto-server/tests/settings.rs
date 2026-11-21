//! Runtime settings over REST and the dashboard.

use axum::http::{Method, StatusCode};
use serde_json::json;
use yanto_core::actor::Scope;
use yanto_server::testing::TestApp;

#[tokio::test]
async fn settings_rest_read_patch_reset_and_scopes() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let admin = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Admin).await);
    let read = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Read).await);

    let v = read.get("/api/v1/settings").await;
    assert_eq!(v.status, StatusCode::OK, "{}", v.text());
    let v = v.value();
    assert!(
        v["runtime"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["key"] == "deploy.max_concurrent" && e["value"] == 2)
    );
    assert!(
        v["host"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["label"] == "Port")
    );

    // Read scope cannot write.
    let denied = read
        .json(
            Method::PATCH,
            "/api/v1/settings",
            &json!({"deploy.max_concurrent": 3}),
        )
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    let ok = admin
        .json(
            Method::PATCH,
            "/api/v1/settings",
            &json!({"deploy.max_concurrent": 3}),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    let e = &ok.value()["runtime"][0];
    assert_eq!(
        (e["value"].clone(), e["overridden"].clone()),
        (json!(3), json!(true))
    );

    let bad = admin
        .json(
            Method::PATCH,
            "/api/v1/settings",
            &json!({"deploy.max_concurrent": 999}),
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let unknown = admin
        .json(Method::PATCH, "/api/v1/settings", &json!({"nope": 1}))
        .await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY);

    let reset = admin
        .json(
            Method::PATCH,
            "/api/v1/settings",
            &json!({"deploy.max_concurrent": null}),
        )
        .await;
    assert_eq!(reset.value()["runtime"][0]["overridden"], false);

    // A Member's token is refused.
    let m = app.member_client("mia", &[]).await;
    assert_eq!(m.get("/settings").await.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn settings_dashboard_page_saves_and_reports_errors() {
    let app = TestApp::new().await;
    let o = app.owner_client().await;
    let page = o.get("/settings").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.text();
    assert!(html.contains("Runtime") && html.contains("/settings/api-tokens"));
    assert!(html.contains("Authorized Keys for the Unix user"));
    assert!(!html.contains("onclick") && !html.contains("<script>"));
    assert!(o.get("/").await.text().contains("href=\"/settings\""));

    let saved = o
        .post_form(
            "/settings",
            &[
                ("deploy.max_concurrent", "4"),
                ("runtime.trusted_proxies", "10.0.0.0/8\n192.168.1.1"),
                ("deploy.retention_per_project", ""),
            ],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
    assert!(saved.text().contains("Saved"));
    assert!(saved.text().contains("10.0.0.0/8"));

    let bad = o
        .post_form("/settings", &[("deploy.max_concurrent", "0")])
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let nan = o
        .post_form("/settings", &[("deploy.max_concurrent", "x")])
        .await;
    assert_eq!(nan.status, StatusCode::UNPROCESSABLE_ENTITY);
}
