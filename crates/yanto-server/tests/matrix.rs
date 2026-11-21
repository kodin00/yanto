//! Adapter-level permission matrix: every M1 REST and dashboard route against every
//! principal. The service-level matrix (`yanto-core`, every operation x actor profile)
//! proves the rules; this proves each adapter route is wired to a service that enforces them,
//! and that no route is reachable with the wrong kind of credential.

use axum::http::{Method, StatusCode};
use serde_json::json;
use yanto_core::actor::{Permission, Scope};
use yanto_server::testing::{TestApp, TestClient};

struct Rest {
    method: Method,
    path: String,
    body: serde_json::Value,
    /// Who may call it: [owner-read, owner-write, owner-admin, member-read, member-write, member-admin].
    allowed: [bool; 6],
}

fn rest_routes(bob: &str) -> Vec<Rest> {
    let any = [true; 6];
    let owner_admin = [false, false, true, false, false, false];
    let owner_read_up = [true, true, true, false, false, false];
    let r = |method: Method, path: &str, body: serde_json::Value, allowed| Rest {
        method,
        path: path.replace("{bob}", bob),
        body,
        allowed,
    };
    vec![
        r(Method::GET, "/api/v1/me", json!({}), any),
        r(Method::GET, "/api/v1/audit", json!({}), any),
        // M3. Containers: scope and role rules here; the per-container rule is in the core tests.
        r(Method::GET, "/api/v1/containers", json!({}), any),
        r(Method::GET, "/api/v1/containers/web", json!({}), any),
        r(Method::GET, "/api/v1/containers/web/logs", json!({}), any),
        r(
            Method::GET,
            "/api/v1/containers/web/logs/stream",
            json!({}),
            any,
        ),
        r(
            Method::POST,
            "/api/v1/containers/web/start",
            json!({}),
            [false, true, true, false, true, true],
        ),
        r(
            Method::POST,
            "/api/v1/containers/web/stop",
            json!({}),
            [false, false, true, false, false, true],
        ),
        r(
            Method::POST,
            "/api/v1/containers/web/restart",
            json!({}),
            [false, false, true, false, false, true],
        ),
        r(Method::GET, "/api/v1/health", json!({}), any),
        r(
            Method::GET,
            "/api/v1/system/usage",
            json!({}),
            owner_read_up,
        ),
        r(
            Method::GET,
            "/api/v1/system/cleanup/preview",
            json!({}),
            owner_read_up,
        ),
        r(
            Method::POST,
            "/api/v1/system/cleanup",
            json!({}),
            owner_admin,
        ),
        r(Method::GET, "/api/v1/users", json!({}), owner_admin),
        r(Method::GET, "/api/v1/users/{bob}", json!({}), owner_admin),
        r(
            Method::POST,
            "/api/v1/users",
            json!({"username": "newbie"}),
            owner_admin,
        ),
        r(
            Method::PUT,
            "/api/v1/users/{bob}/access",
            json!({"grants": []}),
            owner_admin,
        ),
        r(
            Method::PATCH,
            "/api/v1/users/{bob}/status",
            json!({"status": "disabled"}),
            owner_admin,
        ),
        r(
            Method::POST,
            "/api/v1/users/{bob}/reset-link",
            json!({}),
            owner_admin,
        ),
        r(
            Method::DELETE,
            "/api/v1/users/{bob}",
            json!({}),
            owner_admin,
        ),
    ]
}

#[tokio::test]
async fn rest_routes_against_every_principal() {
    let mut failures = Vec::new();
    let routes_for = |bob: &str| rest_routes(bob);
    let n = routes_for("x").len();
    for i in 0..n {
        // Fresh app per route so destructive routes (DELETE) do not affect later cases.
        for (p, (who, scope)) in [
            ("owner", Scope::Read),
            ("owner", Scope::Write),
            ("owner", Scope::Admin),
            ("member", Scope::Read),
            ("member", Scope::Write),
            ("member", Scope::Admin),
        ]
        .into_iter()
        .enumerate()
        {
            let app = TestApp::new().await;
            app.core.docker.add_container("web", "running", &[]);
            let owner = app.owner_user().await;
            let alice = app
                .core
                .create_member("alice", &[("prj_1", &[Permission::Deploy])])
                .await;
            let bob = app.core.create_member("bob", &[]).await;
            let route = &routes_for(&bob.id)[i];
            let user = if who == "owner" { &owner } else { &alice };
            let c = app.client().with_bearer(&app.bearer(user, scope).await);
            let res = c.json(route.method.clone(), &route.path, &route.body).await;
            let label = format!("{} {} as {who}-{scope}", route.method, route.path);
            if route.allowed[p] {
                if matches!(res.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                    failures.push(format!("{label}: expected success, got {}", res.status));
                }
            } else if res.status != StatusCode::FORBIDDEN {
                failures.push(format!("{label}: expected 403, got {}", res.status));
            }
            // Anonymous callers are always 401, whatever the route.
            if p == 0 {
                let anon = app
                    .client()
                    .json(route.method.clone(), &route.path, &route.body)
                    .await;
                if anon.status != StatusCode::UNAUTHORIZED {
                    failures.push(format!(
                        "{} {} anonymous: expected 401, got {}",
                        route.method, route.path, anon.status
                    ));
                }
                // A browser session on a REST route is anonymous too.
                let cookie = app
                    .owner_client()
                    .await
                    .json(route.method.clone(), &route.path, &route.body)
                    .await;
                if cookie.status != StatusCode::UNAUTHORIZED {
                    failures.push(format!(
                        "{} {} with cookie only: expected 401, got {}",
                        route.method, route.path, cookie.status
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "REST matrix violations:\n{}",
        failures.join("\n")
    );
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Who {
    Owner,
    Member,
}

struct Dash {
    method: Method,
    path: &'static str,
    form: &'static [(&'static str, &'static str)],
    htmx: bool,
    /// Whether a Member may use it (the Owner always may).
    member_ok: bool,
}

fn dash_routes() -> Vec<Dash> {
    let d = |method, path, form, htmx, member_ok| Dash {
        method,
        path,
        form,
        htmx,
        member_ok,
    };
    vec![
        d(Method::GET, "/", &[], false, true),
        d(Method::GET, "/audit", &[], false, true),
        d(Method::GET, "/settings/api-tokens", &[], false, true),
        d(
            Method::POST,
            "/settings/api-tokens",
            &[("name", "m1"), ("scope", "read")],
            false,
            true,
        ),
        d(
            Method::DELETE,
            "/settings/api-tokens/tok_missing",
            &[],
            true,
            true,
        ),
        d(Method::GET, "/settings/sessions", &[], false, true),
        d(
            Method::DELETE,
            "/settings/sessions/missing",
            &[],
            true,
            true,
        ),
        d(
            Method::POST,
            "/settings/password",
            &[
                ("current_password", "x"),
                ("new_password", "y"),
                ("confirm", "y"),
            ],
            false,
            true,
        ),
        // M3. Members may call the container routes (the core answers 404 for containers they
        // cannot see) but never /system. Exec is a page plus a WebSocket upgrade, no REST.
        d(Method::GET, "/containers", &[], false, true),
        d(Method::GET, "/containers/rows", &[], false, true),
        d(Method::GET, "/containers/web", &[], false, true),
        d(Method::GET, "/containers/web/logs", &[], false, true),
        d(Method::GET, "/containers/web/logs/stream", &[], false, true),
        d(Method::GET, "/containers/web/exec", &[], false, true),
        d(Method::GET, "/containers/web/exec/ws", &[], false, true),
        d(Method::POST, "/containers/web/start", &[], true, true),
        d(Method::POST, "/containers/web/stop", &[], true, true),
        d(Method::POST, "/containers/web/restart", &[], true, true),
        d(Method::GET, "/system", &[], false, false),
        d(
            Method::POST,
            "/system/cleanup",
            &[("build_cache", "on")],
            false,
            false,
        ),
        d(Method::GET, "/users", &[], false, false),
        d(Method::GET, "/users/{bob}", &[], false, false),
        d(
            Method::POST,
            "/users",
            &[("username", "newbie")],
            false,
            false,
        ),
        d(Method::PUT, "/users/{bob}/access", &[], true, false),
        d(
            Method::PATCH,
            "/users/{bob}/status",
            &[("status", "disabled")],
            true,
            false,
        ),
        d(Method::POST, "/users/{bob}/reset-link", &[], true, false),
        d(Method::DELETE, "/users/{bob}", &[], true, false),
    ]
}

async fn call(c: &TestClient, d: &Dash, bob: &str) -> axum::http::StatusCode {
    let path = d.path.replace("{bob}", bob);
    if d.htmx {
        c.htmx(d.method.clone(), &path, d.form).await.status
    } else if d.method == Method::GET {
        c.get(&path).await.status
    } else {
        c.form(d.method.clone(), &path, d.form).await.status
    }
}

#[tokio::test]
async fn dashboard_routes_against_every_principal() {
    let mut failures = Vec::new();
    for i in 0..dash_routes().len() {
        for who in [Who::Owner, Who::Member] {
            let app = TestApp::new().await;
            app.core.docker.add_container("web", "running", &[]);
            let owner = app.owner_client().await;
            let alice = app
                .member_client("alice", &[("prj_1", &[Permission::Deploy])])
                .await;
            let bob = app.core.create_member("bob", &[]).await;
            let routes = dash_routes();
            let d = &routes[i];
            let label = format!("{} {} as {who:?}", d.method, d.path);

            // Anonymous: redirected to login (or 401 + HX-Redirect for htmx), never served.
            let anon = call(&app.client(), d, &bob.id).await;
            if !matches!(
                anon,
                StatusCode::SEE_OTHER | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) {
                failures.push(format!("{} {} anonymous: got {anon}", d.method, d.path));
            }
            // A bearer token is not a browser credential.
            let token = app.bearer(&app.owner_user().await, Scope::Admin).await;
            let bearer = call(&app.client().with_bearer(&token), d, &bob.id).await;
            if !matches!(
                bearer,
                StatusCode::SEE_OTHER | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) {
                failures.push(format!("{} {} bearer-only: got {bearer}", d.method, d.path));
            }

            let status = match who {
                Who::Owner => call(&owner, d, &bob.id).await,
                Who::Member => call(&alice, d, &bob.id).await,
            };
            let allowed = who == Who::Owner || d.member_ok;
            if allowed {
                if matches!(
                    status,
                    StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED | StatusCode::SEE_OTHER
                ) && !(d.path == "/settings/password" || d.path == "/logout")
                {
                    failures.push(format!("{label}: expected success, got {status}"));
                }
            } else if status != StatusCode::FORBIDDEN {
                failures.push(format!("{label}: expected 403, got {status}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "dashboard matrix violations:\n{}",
        failures.join("\n")
    );
}
