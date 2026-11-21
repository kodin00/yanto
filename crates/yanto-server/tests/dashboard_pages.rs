//! Dashboard pages: users and access, API tokens, audit log, per role.

use axum::http::{Method, StatusCode};
use yanto_core::actor::Permission;
use yanto_server::testing::TestApp;

fn first_token_on_page(body: &str) -> String {
    let marker = "<code class=\"link\">";
    let s = body.find(marker).expect("token box") + marker.len();
    body[s..s + body[s..].find('<').unwrap()].to_owned()
}

#[tokio::test]
async fn owner_manages_members_through_htmx() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;

    let dup = owner.post_form("/users", &[("username", "bad name")]).await;
    assert_eq!(dup.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(dup.text().contains("username may contain"));
    owner.post_form("/users", &[("username", "alice")]).await;
    let conflict = owner.post_form("/users", &[("username", "ALICE")]).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);

    let list = owner.get("/users").await;
    assert_eq!(list.status, StatusCode::OK);
    let html = list.text();
    assert!(html.contains("alice") && html.contains("invited"));
    let id = app
        .core
        .services
        .users
        .list(&yanto_core::actor::Actor::cli())
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.username == "alice")
        .unwrap()
        .id;

    // Access editor: add a Project with two permissions.
    let detail = owner.get(&format!("/users/{id}")).await;
    assert_eq!(detail.status, StatusCode::OK);
    assert!(detail.text().contains("hx-put"));
    let saved = owner
        .htmx(
            Method::PUT,
            &format!("/users/{id}/access"),
            &[
                ("new_project", "prj_1"),
                ("new_perm", "deploy"),
                ("new_perm", "secrets"),
            ],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK);
    assert!(saved.text().contains("Saved"));
    let user = app
        .core
        .services
        .users
        .get(&yanto_core::actor::Actor::cli(), &id)
        .await
        .unwrap();
    assert_eq!(user.grants[0].project_id, "prj_1");
    assert_eq!(user.grants[0].permissions, ["deploy", "secrets"]);

    // Edit: keep the row, drop `secrets`; the checkbox form posts only what is checked.
    owner
        .htmx(
            Method::PUT,
            &format!("/users/{id}/access"),
            &[
                ("keep", "prj_1"),
                ("perm", "prj_1|deploy"),
                ("perm", "prj_1|config"),
            ],
        )
        .await;
    let user = app
        .core
        .services
        .users
        .get(&yanto_core::actor::Actor::cli(), &id)
        .await
        .unwrap();
    assert_eq!(user.grants[0].permissions, ["deploy", "config"]);
    // Unchecking "keep" removes the Project.
    owner
        .htmx(Method::PUT, &format!("/users/{id}/access"), &[])
        .await;
    assert!(
        app.core
            .services
            .users
            .get(&yanto_core::actor::Actor::cli(), &id)
            .await
            .unwrap()
            .grants
            .is_empty()
    );
    // Invalid permission names are a 422 toast.
    let bad = owner
        .htmx(
            Method::PUT,
            &format!("/users/{id}/access"),
            &[("new_project", "p"), ("new_perm", "tasks")],
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(bad.header("hx-retarget"), Some("#toast"));

    // Status, reset link, delete.
    let dis = owner
        .htmx(
            Method::PATCH,
            &format!("/users/{id}/status"),
            &[("status", "disabled")],
        )
        .await;
    assert_eq!(dis.header("hx-refresh"), Some("true"));
    assert!(
        owner
            .get(&format!("/users/{id}"))
            .await
            .text()
            .contains("Enable")
    );
    owner
        .htmx(
            Method::PATCH,
            &format!("/users/{id}/status"),
            &[("status", "active")],
        )
        .await;
    let link = owner
        .htmx(Method::POST, &format!("/users/{id}/reset-link"), &[])
        .await;
    assert_eq!(link.status, StatusCode::OK);
    assert!(link.text().contains("/account/setup#token="));
    assert_eq!(link.header("cache-control"), Some("no-store"));
    let del = owner
        .htmx(Method::DELETE, &format!("/users/{id}"), &[])
        .await;
    assert_eq!(del.header("hx-redirect"), Some("/users"));
    assert_eq!(
        owner.get(&format!("/users/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn owner_detail_page_offers_no_edit_controls() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    let html = owner.get("/users").await.text();
    let id = format!(
        "usr_{}",
        html.split("href=\"/users/usr_")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
    );
    let page = owner.get(&format!("/users/{id}")).await.text();
    assert!(page.contains("owner-reset") && !page.contains("hx-delete"));
    let del = owner
        .htmx(Method::DELETE, &format!("/users/{id}"), &[])
        .await;
    assert_eq!(
        del.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the Owner cannot be deleted"
    );
}

#[tokio::test]
async fn members_cannot_reach_owner_pages_or_actions() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    let alice = app
        .member_client("alice", &[("prj_1", &[Permission::Deploy])])
        .await;
    owner.post_form("/users", &[("username", "bob")]).await;
    for path in ["/users", "/users/usr_x"] {
        let res = alice.get(path).await;
        assert_eq!(res.status, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(
        alice
            .post_form("/users", &[("username", "evil")])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let del = alice.htmx(Method::DELETE, "/users/usr_x", &[]).await;
    assert_eq!(del.status, StatusCode::FORBIDDEN);
    assert_eq!(del.header("hx-retarget"), Some("#toast"));
    // The nav hides what the Member cannot use.
    let home = alice.get("/").await.text();
    assert!(!home.contains("href=\"/users\"") && home.contains("href=\"/audit\""));
}

#[tokio::test]
async fn api_token_page_shows_the_secret_once_and_the_token_works() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    let created = owner
        .post_form(
            "/settings/api-tokens",
            &[
                ("name", "ci"),
                ("scope", "write"),
                ("expires_in_days", "30"),
            ],
        )
        .await;
    assert_eq!(created.status, StatusCode::OK);
    assert_eq!(created.header("cache-control"), Some("no-store"));
    let token = first_token_on_page(&created.text());
    assert!(token.starts_with("yat_"));

    let again = owner.get("/settings/api-tokens").await.text();
    assert!(!again.contains(&token), "token is never shown again");
    assert!(again.contains("ci") && again.contains("write"));

    let me = app.client().with_bearer(&token).get("/api/v1/me").await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.value()["token"]["scope"], "write");

    let dup = owner
        .post_form("/settings/api-tokens", &[("name", "ci"), ("scope", "read")])
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    let bad_scope = owner
        .post_form("/settings/api-tokens", &[("name", "x"), ("scope", "root")])
        .await;
    assert_eq!(bad_scope.status, StatusCode::UNPROCESSABLE_ENTITY);
    let bad_days = owner
        .post_form(
            "/settings/api-tokens",
            &[
                ("name", "x"),
                ("scope", "read"),
                ("expires_in_days", "soon"),
            ],
        )
        .await;
    assert_eq!(bad_days.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn member_tokens_are_private_and_the_owner_can_revoke_them() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    let alice = app
        .member_client("alice", &[("prj_1", &[Permission::Deploy])])
        .await;
    let bob = app.member_client("bob", &[]).await;

    let made = alice
        .post_form(
            "/settings/api-tokens",
            &[("name", "alice-ci"), ("scope", "admin")],
        )
        .await;
    assert_eq!(
        made.status,
        StatusCode::OK,
        "Members may create their own tokens (D29)"
    );
    let token = first_token_on_page(&made.text());

    assert!(
        !bob.get("/settings/api-tokens")
            .await
            .text()
            .contains("alice-ci")
    );
    let owner_view = owner.get("/settings/api-tokens").await.text();
    assert!(owner_view.contains("alice-ci") && owner_view.contains("<th>User</th>"));

    // Even an admin-scope token cannot exceed its user: Owner-only REST is closed to it.
    let c = app.client().with_bearer(&token);
    assert_eq!(c.get("/api/v1/users").await.status, StatusCode::FORBIDDEN);

    let id = owner_view
        .split("/settings/api-tokens/tok_")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let id = format!("tok_{id}");
    assert_eq!(
        bob.htmx(Method::DELETE, &format!("/settings/api-tokens/{id}"), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        owner
            .htmx(Method::DELETE, &format!("/settings/api-tokens/{id}"), &[])
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(c.get("/api/v1/me").await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn audit_page_records_auth_events_and_scopes_members() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    owner.post_form("/users", &[("username", "alice")]).await;
    let alice = app.member_client("carol", &[("prj_1", &[])]).await;

    let page = owner.get("/audit").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.text();
    for action in [
        "auth.setup_owner",
        "auth.login",
        "user.create",
        "auth.account_setup",
    ] {
        assert!(html.contains(action), "{action} missing from audit page");
    }
    let mine = alice.get("/audit").await;
    assert_eq!(mine.status, StatusCode::OK);
    assert!(
        mine.text().contains("No entries"),
        "no entries carry carol's project yet"
    );
    let other = alice.get("/audit?project_id=prj_2").await;
    assert_eq!(other.status, StatusCode::NOT_FOUND);
    assert_eq!(
        owner.get("/audit?cursor=junk").await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}
