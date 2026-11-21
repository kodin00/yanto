//! REST `/api/v1`: bearer auth, scopes, error envelope, users, audit.

use axum::http::{Method, StatusCode};
use serde_json::json;
use yanto_core::actor::{Permission, Scope};
use yanto_server::testing::TestApp;

#[tokio::test]
async fn whoami_needs_a_valid_bearer_token() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let token = app.bearer(&owner, Scope::Read).await;

    let ok = app.client().with_bearer(&token).get("/api/v1/me").await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.header("cache-control"), Some("no-store"));
    let v = ok.value();
    assert_eq!(v["username"], "owner");
    assert_eq!(v["role"], "owner");
    assert_eq!(v["token"]["scope"], "read");
    assert!(
        v.to_string().find(&token).is_none(),
        "the token is never echoed"
    );

    for c in [
        app.client(),
        app.client().with_bearer("yat_nope"),
        app.client().with_bearer(""),
    ] {
        let res = c.get("/api/v1/me").await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED);
        assert_eq!(res.header("www-authenticate"), Some("Bearer"));
        assert_eq!(res.value()["error"]["code"], "unauthenticated");
    }
}

#[tokio::test]
async fn unknown_endpoints_and_bad_bodies_use_the_error_envelope() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let c = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Admin).await);
    let nf = c.get("/api/v1/nope").await;
    assert_eq!(nf.status, StatusCode::NOT_FOUND);
    assert_eq!(nf.value()["error"]["code"], "not_found");
    let bad = c
        .send(
            Method::POST,
            "/api/v1/users",
            Some(("application/json", "{not json".into())),
            &[],
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(bad.value()["error"]["code"], "bad_request");
    let missing = c.json(Method::POST, "/api/v1/users", &json!({})).await;
    assert_eq!(missing.status, StatusCode::UNPROCESSABLE_ENTITY);
    let invalid = c
        .json(Method::POST, "/api/v1/users", &json!({"username": "a b"}))
        .await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
    let v = invalid.value();
    assert_eq!(v["error"]["code"], "validation_failed");
    assert_eq!(v["error"]["fields"][0]["field"], "username");
    let bad_perm = c
        .json(
            Method::POST,
            "/api/v1/users",
            &json!({"username": "x", "grants": [{"project_id": "p1", "permissions": ["tasks"]}]}),
        )
        .await;
    assert_eq!(bad_perm.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn user_endpoints_require_admin_scope_and_the_owner() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let alice = app
        .core
        .create_member("alice", &[("prj_1", &[Permission::Deploy])])
        .await;
    for scope in [Scope::Read, Scope::Write] {
        let c = app.client().with_bearer(&app.bearer(&owner, scope).await);
        assert_eq!(
            c.get("/api/v1/users").await.status,
            StatusCode::FORBIDDEN,
            "{scope}"
        );
    }
    // A Member's admin token is still a Member.
    let m = app
        .client()
        .with_bearer(&app.bearer(&alice, Scope::Admin).await);
    let res = m.get("/api/v1/users").await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(res.value()["error"]["code"], "forbidden");
    let admin = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Admin).await);
    assert_eq!(admin.get("/api/v1/users").await.status, StatusCode::OK);
}

#[tokio::test]
async fn user_lifecycle_over_rest() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    // Grants need real Projects since 0201.
    app.core.ensure_project("prj_7").await;
    app.core.ensure_project("prj_8").await;
    let c = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Admin).await);

    let created = c
        .json(
            Method::POST,
            "/api/v1/users",
            &json!({"username": "dana", "grants": [{"project_id": "prj_7", "permissions": ["deploy", "backups"]}]}),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let v = created.value();
    let id = v["user"]["id"].as_str().unwrap().to_owned();
    assert_eq!(v["user"]["status"], "invited");
    assert_eq!(
        v["user"]["grants"][0]["permissions"],
        json!(["deploy", "backups"])
    );
    assert!(
        v["invite_url"]
            .as_str()
            .unwrap()
            .contains("/account/setup#token=")
    );
    assert!(v["user"].get("password_hash").is_none());

    let list = c.get("/api/v1/users").await.value();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(
        c.get(&format!("/api/v1/users/{id}")).await.value()["username"],
        "dana"
    );
    assert_eq!(
        c.json(Method::POST, "/api/v1/users", &json!({"username": "DANA"}))
            .await
            .status,
        StatusCode::CONFLICT
    );

    let upd = c
        .json(
            Method::PUT,
            &format!("/api/v1/users/{id}/access"),
            &json!({"grants": [{"project_id": "prj_8", "permissions": []}]}),
        )
        .await;
    assert_eq!(upd.status, StatusCode::OK);
    assert_eq!(upd.value()["grants"][0]["project_id"], "prj_8");

    let dis = c
        .json(
            Method::PATCH,
            &format!("/api/v1/users/{id}/status"),
            &json!({"status": "disabled"}),
        )
        .await;
    assert_eq!(dis.value()["status"], "disabled");
    let bad = c
        .json(
            Method::PATCH,
            &format!("/api/v1/users/{id}/status"),
            &json!({"status": "invited"}),
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    c.json(
        Method::PATCH,
        &format!("/api/v1/users/{id}/status"),
        &json!({"status": "active"}),
    )
    .await;

    let reset = c
        .json(
            Method::POST,
            &format!("/api/v1/users/{id}/reset-link"),
            &json!({}),
        )
        .await;
    assert_eq!(reset.status, StatusCode::OK);
    assert!(reset.value()["url"].as_str().unwrap().contains("#token="));

    assert_eq!(
        c.send(Method::DELETE, &format!("/api/v1/users/{id}"), None, &[])
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        c.get(&format!("/api/v1/users/{id}")).await.status,
        StatusCode::NOT_FOUND
    );

    // Every mutation is audited as a REST call by this token.
    let audit = c.get("/api/v1/audit?limit=100").await.value();
    let rows = audit["entries"].as_array().unwrap();
    for action in [
        "user.create",
        "user.access_update",
        "user.status_update",
        "user.reset_link",
        "user.delete",
    ] {
        let row = rows
            .iter()
            .find(|r| r["action"] == action)
            .unwrap_or_else(|| panic!("{action} not audited"));
        assert_eq!(row["channel"], "rest");
        assert_eq!(row["actor_type"], "api_token");
    }
    assert!(
        !audit.to_string().contains("#token="),
        "audit never holds links"
    );
}

#[tokio::test]
async fn audit_endpoint_is_scoped_paginated_and_read_only_scope_suffices() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let alice = app.core.create_member("alice", &[("prj_1", &[])]).await;
    for _ in 0..4 {
        app.core
            .services
            .auth
            .login(&app.core.origin(), "owner", "wrong-password-1", None)
            .await
            .ok();
    }
    let read = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Read).await);
    let first = read.get("/api/v1/audit?limit=2").await;
    assert_eq!(first.status, StatusCode::OK);
    let v = first.value();
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
    let cursor = v["next_cursor"].as_str().unwrap();
    let second = read
        .get(&format!("/api/v1/audit?limit=2&cursor={cursor}"))
        .await
        .value();
    assert_ne!(second["entries"][0]["id"], v["entries"][0]["id"]);
    assert_eq!(
        read.get("/api/v1/audit?limit=x").await.status,
        StatusCode::BAD_REQUEST
    );

    let m = app
        .client()
        .with_bearer(&app.bearer(&alice, Scope::Read).await);
    assert_eq!(
        m.get("/api/v1/audit").await.value()["entries"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        m.get("/api/v1/audit?project_id=prj_2").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn repeated_bad_tokens_are_rate_limited_per_ip() {
    let app = TestApp::new().await;
    app.owner_user().await;
    let c = app.client().with_ip("192.0.2.77").with_bearer("yat_wrong");
    for _ in 0..20 {
        assert_eq!(c.get("/api/v1/me").await.status, StatusCode::UNAUTHORIZED);
    }
    let blocked = c.get("/api/v1/me").await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(blocked.header("retry-after").is_some());
    assert_eq!(blocked.value()["error"]["code"], "rate_limited");
    // Another client address is not affected.
    let other = app.client().with_ip("192.0.2.78").with_bearer("yat_wrong");
    assert_eq!(
        other.get("/api/v1/me").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn token_management_is_not_available_over_rest() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let c = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Admin).await);
    for (m, p) in [
        (Method::POST, "/api/v1/tokens"),
        (Method::GET, "/api/v1/tokens"),
        (Method::POST, "/api/v1/login"),
        (Method::GET, "/api/v1/sessions"),
    ] {
        assert_eq!(
            c.json(m, p, &json!({})).await.status,
            StatusCode::NOT_FOUND,
            "{p}"
        );
    }
}
