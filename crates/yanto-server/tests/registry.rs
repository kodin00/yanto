//! Hosted Registry over REST and the dashboard, against the fake docker/registry ports.

use axum::http::{Method, StatusCode};
use serde_json::json;
use yanto_core::actor::Scope;
use yanto_core::testing::command::Reply;
use yanto_core::testing::registry_api::fake_digest;
use yanto_server::testing::{TestApp, TestClient};

const PINNED: &str =
    "registry@sha256:1111111111111111111111111111111111111111111111111111111111111111";

async fn admin(app: &TestApp) -> TestClient {
    let owner = app.owner_user().await;
    app.client()
        .with_bearer(&app.bearer(&owner, Scope::Admin).await)
}

async fn enabled(app: &TestApp, c: &TestClient) {
    app.core
        .commands
        .on("image inspect", Reply::stdout(format!(r#"["{PINNED}"]"#)));
    let res = c
        .json(
            Method::POST,
            "/api/v1/registry/enable",
            &json!({"listen_port": 5055}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text());
}

#[tokio::test]
async fn rest_lifecycle_users_browse_delete_and_gc() {
    let app = TestApp::new().await;
    let c = admin(&app).await;
    let st = c.get("/api/v1/registry").await.value();
    assert_eq!(st["enabled"], false);
    assert_eq!(st["health"]["state"], "disabled");

    enabled(&app, &c).await;
    let st = c.get("/api/v1/registry").await.value();
    assert_eq!(st["enabled"], true);
    assert_eq!(st["image_host"], "127.0.0.1:5055");
    assert_eq!(st["health"]["state"], "healthy");

    // A generated password is returned once and never listed again.
    let res = c
        .json(
            Method::POST,
            "/api/v1/registry/users",
            &json!({"username": "ci"}),
        )
        .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
    let v = res.value();
    let id = v["user"]["id"].as_str().unwrap().to_owned();
    let pw = v["password"]
        .as_str()
        .expect("generated password")
        .to_owned();
    assert_eq!(pw.len(), 24);
    let list = c.get("/api/v1/registry/users").await;
    assert!(!list.text().contains(&pw) && !list.text().contains("password_hash"));
    assert_eq!(list.value()["users"][0]["username"], "ci");
    let res = c
        .json(
            Method::PUT,
            &format!("/api/v1/registry/users/{id}/password"),
            &json!({}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.value()["password"].as_str().unwrap().len(), 24);
    let res = c
        .json(
            Method::POST,
            "/api/v1/registry/users",
            &json!({"username": "ci"}),
        )
        .await;
    assert_eq!(res.status, StatusCode::CONFLICT);
    let res = c
        .json(
            Method::POST,
            "/api/v1/registry/users",
            &json!({"username": "Bad:Name"}),
        )
        .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Repos and tags; repository names with slashes are one percent-encoded segment.
    let d = fake_digest("a");
    app.core.registry_api.push("team/app", "v1", &d, 10);
    app.core.registry_api.push("team/app", "latest", &d, 10);
    let repos = c.get("/api/v1/registry/repos").await.value();
    assert_eq!(repos["repos"][0], "team/app");
    let tags = c
        .get("/api/v1/registry/repos/team%2Fapp/tags")
        .await
        .value();
    assert_eq!(tags["tags"].as_array().unwrap().len(), 2);
    assert_eq!(tags["tags"][0]["digest"], d);
    let preview = c
        .get("/api/v1/registry/repos/team%2Fapp/tags/v1")
        .await
        .value();
    assert_eq!(preview["also_removed"][0], "latest");
    assert_eq!(
        c.get("/api/v1/registry/repos/Bad%20Name/tags").await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let del = c
        .json(
            Method::DELETE,
            "/api/v1/registry/repos/team%2Fapp/tags/v1",
            &json!({}),
        )
        .await;
    assert_eq!(del.status, StatusCode::OK, "{}", del.text());
    assert!(app.core.registry_api.tags_of("team/app").is_empty());

    // GC: 202 and a pollable status.
    let res = c
        .json(
            Method::POST,
            "/api/v1/registry/gc",
            &json!({"delete_untagged": true}),
        )
        .await;
    assert_eq!(res.status, StatusCode::ACCEPTED, "{}", res.text());
    app.core.services.registry.gc_wait().await;
    let g = c.get("/api/v1/registry/gc").await.value();
    assert_eq!(g["state"], "done", "{g}");

    let res = c
        .json(
            Method::DELETE,
            &format!("/api/v1/registry/users/{id}"),
            &json!({}),
        )
        .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    let res = c
        .json(
            Method::POST,
            "/api/v1/registry/disable",
            &json!({"delete_data": "nope"}),
        )
        .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    let res = c
        .json(Method::POST, "/api/v1/registry/disable", &json!({}))
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.value()["enabled"], false);
    // Browsing a disabled registry is a 409.
    assert_eq!(
        c.get("/api/v1/registry/repos").await.status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn dashboard_page_users_browser_and_guidance() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    let page = owner.get("/registry").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text().contains("Enable registry"));
    assert!(!page.text().contains("onclick") && !page.text().contains("<script>"));

    app.core
        .commands
        .on("image inspect", Reply::stdout(format!(r#"["{PINNED}"]"#)));
    let res = owner
        .post_form("/registry/enable", &[("listen_port", "5055")])
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.text());
    let page = owner.get("/registry").await;
    assert!(page.text().contains("127.0.0.1:5055") && page.text().contains("healthy"));
    assert!(page.text().contains("Garbage collection"));

    // The one-time password appears in the response that created it, uncached, and nowhere else.
    let res = owner
        .post_form("/registry/users", &[("username", "ci"), ("password", "")])
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.header("cache-control"), Some("no-store"));
    let text = res.text();
    let start = text.find("<pre>").unwrap() + 5;
    let pw = &text[start..start + 24];
    assert!(!owner.get("/registry").await.text().contains(pw));
    let bad = owner
        .post_form(
            "/registry/users",
            &[("username", "Bad Name"), ("password", "")],
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(bad.text().contains("Bad Name"), "typed value kept");

    // Fragments.
    let d = fake_digest("x");
    app.core.registry_api.push("team/app", "v1", &d, 2048);
    app.core.registry_api.push("team/app", "stable", &d, 2048);
    let repos = owner.htmx(Method::GET, "/registry/repos", &[]).await;
    assert!(repos.text().contains("/registry/repos/team%2Fapp/tags"));
    let tags = owner
        .htmx(Method::GET, "/registry/repos/team%2Fapp/tags", &[])
        .await;
    assert!(tags.text().contains("127.0.0.1:5055/team/app:v1") && tags.text().contains("KiB"));
    let confirm = owner
        .htmx(Method::GET, "/registry/repos/team%2Fapp/tags/v1", &[])
        .await;
    assert!(
        confirm.text().contains("stable"),
        "sibling tags are listed first"
    );
    let del = owner
        .htmx(Method::DELETE, "/registry/repos/team%2Fapp/tags/v1", &[])
        .await;
    assert_eq!(del.status, StatusCode::OK);
    assert_eq!(del.header("hx-trigger"), Some("registry-changed"));
    assert!(app.core.registry_api.tags_of("team/app").is_empty());

    let gc = owner
        .htmx(Method::POST, "/registry/gc", &[("delete_untagged", "1")])
        .await;
    assert_eq!(gc.status, StatusCode::OK);
    app.core.services.registry.gc_wait().await;
    assert!(
        owner
            .htmx(Method::GET, "/registry/gc", &[])
            .await
            .text()
            .contains("finished")
    );

    let guide = owner.get("/registry/remote-push").await;
    assert!(
        guide.text().contains("not verified") || guide.text().contains("not</strong> verified")
    );
    assert!(guide.text().contains("Traefik"));
    let host = owner
        .post_form(
            "/registry/settings",
            &[("public_host", "registry.example.com")],
        )
        .await;
    assert_eq!(host.status, StatusCode::SEE_OTHER);
    assert!(
        owner
            .get("/registry/remote-push")
            .await
            .text()
            .contains("docker login registry.example.com")
    );

    let off = owner
        .post_form("/registry/disable", &[("delete_data", "")])
        .await;
    assert_eq!(off.status, StatusCode::SEE_OTHER);
    // Nav keeps /registry and /registry-credentials apart.
    let creds = owner.get("/registry-credentials").await.text();
    assert_eq!(creds.matches("aria-current=\"page\"").count(), 1);
}

#[tokio::test]
async fn members_cannot_see_or_use_the_registry() {
    let app = TestApp::new().await;
    let _ = app.owner_user().await;
    let m = app.member_client("mia", &[]).await;
    assert_eq!(m.get("/registry").await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        m.post_form("/registry/enable", &[("listen_port", "5055")])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(!m.get("/").await.text().contains("href=\"/registry\""));
}
