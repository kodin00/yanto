//! Dashboard authentication: setup, login, logout, sessions, CSRF, origin checks, invites.

use axum::http::{Method, StatusCode};
use yanto_core::actor::Actor;
use yanto_core::testing::token_from_link;
use yanto_server::testing::TestApp;

async fn setup_code(app: &TestApp) -> String {
    app.core
        .services
        .auth
        .issue_setup_code(&Actor::cli())
        .await
        .unwrap()
        .code
        .expose()
        .to_owned()
}

#[tokio::test]
async fn anonymous_users_are_sent_to_setup_then_login() {
    let app = TestApp::new().await;
    let c = app.client();
    for path in ["/", "/users", "/audit", "/settings/api-tokens"] {
        let res = c.get(path).await;
        assert_eq!(res.status, StatusCode::SEE_OTHER, "{path}");
        assert_eq!(
            res.location(),
            Some("/setup"),
            "{path} before an Owner exists"
        );
    }
    assert_eq!(c.get("/login").await.location(), Some("/setup"));
    app.owner_user().await;
    assert_eq!(c.get("/").await.location(), Some("/login"));
    assert_eq!(c.get("/setup").await.location(), Some("/login"));
    let page = c.get("/login").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text().contains("name=\"password\""));
}

#[tokio::test]
async fn htmx_requests_without_a_session_get_hx_redirect() {
    let app = TestApp::new().await;
    app.owner_user().await;
    let res = app.client().htmx(Method::GET, "/users", &[]).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert_eq!(res.header("hx-redirect"), Some("/login"));
}

#[tokio::test]
async fn setup_creates_owner_signs_in_and_is_then_closed() {
    let app = TestApp::new().await;
    let code = setup_code(&app).await;
    let c = app.client();
    assert_eq!(c.get("/setup").await.status, StatusCode::OK);

    let bad = c
        .post_form(
            "/setup",
            &[
                ("code", "AAAA-AAAA-AAAA-AAAA"),
                ("username", "boss"),
                ("password", "long-enough-pass"),
                ("confirm", "long-enough-pass"),
            ],
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
    assert!(bad.text().contains("Invalid or expired setup code"));

    let mismatch = c
        .post_form(
            "/setup",
            &[
                ("code", &code),
                ("username", "boss"),
                ("password", "long-enough-pass"),
                ("confirm", "different-pass-1"),
            ],
        )
        .await;
    assert_eq!(mismatch.status, StatusCode::UNPROCESSABLE_ENTITY);

    let ok = c
        .post_form(
            "/setup",
            &[
                ("code", &code),
                ("username", "boss"),
                ("password", "long-enough-pass"),
                ("confirm", "long-enough-pass"),
            ],
        )
        .await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    assert_eq!(ok.location(), Some("/"));
    let cookie = ok.set_cookie_header().unwrap();
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax") && cookie.contains("Path=/")
    );
    assert!(!cookie.contains("Secure"), "plain http dev setup");

    let mut signed_in = app.client();
    signed_in.cookie = ok.session_cookie();
    assert_eq!(signed_in.get("/").await.status, StatusCode::OK);
    // Setup is single-use.
    let again = c
        .post_form(
            "/setup",
            &[
                ("code", &code),
                ("username", "x2"),
                ("password", "long-enough-pass"),
                ("confirm", "long-enough-pass"),
            ],
        )
        .await;
    assert_eq!(again.location(), Some("/login"));
}

#[tokio::test]
async fn login_logout_and_cookie_flags() {
    let app = TestApp::with_public_url("https://yanto.example.com").await;
    let owner = app.owner_user().await;
    let c = app.client().with_origin(Some("https://yanto.example.com"));
    let res = c
        .post_form(
            "/login",
            &[("username", &owner.username), ("password", &owner.password)],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert!(
        res.set_cookie_header().unwrap().contains("Secure"),
        "Secure when public_url is https"
    );

    let mut signed = app.login(&owner).await;
    // `login` uses the default http origin, which public_url mode rejects; use the https one.
    signed.cookie = res.session_cookie();
    let home = signed.get("/").await;
    assert_eq!(home.status, StatusCode::OK);
    signed.csrf = home.csrf_token();
    let signed = signed.with_origin(Some("https://yanto.example.com"));

    let out = signed.post_form("/logout", &[]).await;
    assert_eq!(out.status, StatusCode::SEE_OTHER);
    assert_eq!(out.location(), Some("/login"));
    assert!(out.set_cookie_header().unwrap().contains("Max-Age=0"));
    // The server-side session is gone even though the browser still holds the cookie.
    assert_eq!(signed.get("/").await.location(), Some("/login"));
}

#[tokio::test]
async fn bad_credentials_are_generic_and_rate_limited() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let c = app.client().with_ip("198.51.100.20");
    let wrong = c
        .post_form(
            "/login",
            &[
                ("username", &owner.username),
                ("password", "nope-nope-nope"),
            ],
        )
        .await;
    let unknown = c
        .post_form(
            "/login",
            &[("username", "ghost"), ("password", "nope-nope-nope")],
        )
        .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert!(wrong.text().contains("Invalid username or password"));
    assert!(unknown.text().contains("Invalid username or password"));
    assert!(wrong.session_cookie().is_none());
    for _ in 0..3 {
        c.post_form(
            "/login",
            &[
                ("username", &owner.username),
                ("password", "nope-nope-nope"),
            ],
        )
        .await;
    }
    let locked = c
        .post_form(
            "/login",
            &[("username", &owner.username), ("password", &owner.password)],
        )
        .await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(locked.text().contains("too many attempts"));
}

#[tokio::test]
async fn cross_origin_and_missing_origin_posts_are_rejected_even_when_public() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let fields = [
        ("username", owner.username.as_str()),
        ("password", owner.password.as_str()),
    ];
    let evil = app
        .client()
        .with_origin(Some("http://evil.example"))
        .post_form("/login", &fields)
        .await;
    assert_eq!(evil.status, StatusCode::FORBIDDEN);
    assert!(evil.text().contains("cross-origin"));
    let none = app
        .client()
        .with_origin(None)
        .post_form("/login", &fields)
        .await;
    assert_eq!(none.status, StatusCode::FORBIDDEN);
    let cross_site = app
        .client()
        .send(
            Method::POST,
            "/login",
            Some((
                "application/x-www-form-urlencoded",
                "username=a&password=b".into(),
            )),
            &[("sec-fetch-site", "cross-site")],
        )
        .await;
    assert_eq!(cross_site.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn csrf_token_is_required_on_every_unsafe_dashboard_request() {
    let app = TestApp::new().await;
    let c = app.owner_client().await;
    let no_token = c.clone().without_csrf();
    let res = no_token.post_form("/logout", &[]).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert!(res.text().contains("CSRF"));
    // Wrong token via header and via field.
    let mut wrong = c.clone();
    wrong.csrf = Some("not-the-token".into());
    assert_eq!(
        wrong.post_form("/logout", &[]).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        wrong
            .htmx(Method::DELETE, "/settings/api-tokens/tok_x", &[])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // htmx failure is a toast fragment.
    let htmx = no_token
        .htmx(Method::POST, "/users", &[("username", "x")])
        .await;
    assert_eq!(htmx.status, StatusCode::FORBIDDEN);
    assert_eq!(htmx.header("hx-retarget"), Some("#toast"));
    // Still signed in; a correct form field alone is enough (plain HTML forms).
    let ok = c
        .post_form(
            "/settings/password",
            &[
                ("current_password", "x"),
                ("new_password", "y"),
                ("confirm", "y"),
            ],
        )
        .await;
    assert_ne!(ok.status, StatusCode::FORBIDDEN);
    // GET needs no token.
    assert_eq!(no_token.get("/").await.status, StatusCode::OK);
}

#[tokio::test]
async fn session_cookie_is_ignored_by_the_rest_api() {
    let app = TestApp::new().await;
    let c = app.owner_client().await;
    let res = c.get("/api/v1/me").await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert_eq!(res.header("www-authenticate"), Some("Bearer"));
}

#[tokio::test]
async fn invite_link_flow_over_http() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    let created = owner.post_form("/users", &[("username", "alice")]).await;
    assert_eq!(created.status, StatusCode::OK);
    assert_eq!(created.header("cache-control"), Some("no-store"));
    let body = created.text();
    let start = body.find("/account/setup#token=").expect("link on page");
    let link = &body[start..start + body[start..].find('<').unwrap()];
    let token = token_from_link(link);

    let page = app.client().get("/account/setup").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text().contains("account-setup.js"));
    let preview = app
        .client()
        .post_form("/account/setup/preview", &[("token", &token)])
        .await;
    assert!(preview.text().contains("<strong>alice</strong>"));

    let weak = app
        .client()
        .post_form(
            "/account/setup",
            &[
                ("token", &token),
                ("password", "short"),
                ("confirm", "short"),
            ],
        )
        .await;
    assert_eq!(weak.status, StatusCode::UNPROCESSABLE_ENTITY);
    let ok = app
        .client()
        .post_form(
            "/account/setup",
            &[
                ("token", &token),
                ("password", "alice-secret-pass"),
                ("confirm", "alice-secret-pass"),
            ],
        )
        .await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    let mut alice = app.client();
    alice.cookie = ok.session_cookie();
    assert_eq!(alice.get("/").await.status, StatusCode::OK);
    // Single use.
    let again = app
        .client()
        .post_form(
            "/account/setup",
            &[
                ("token", &token),
                ("password", "another-secret-1"),
                ("confirm", "another-secret-1"),
            ],
        )
        .await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
    assert!(again.text().contains("invalid or has expired"));
    let bad_preview = app
        .client()
        .htmx(Method::POST, "/account/setup/preview", &[("token", &token)])
        .await;
    assert_eq!(bad_preview.header("hx-retarget"), Some("#toast"));
}

#[tokio::test]
async fn password_change_and_session_management() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let a = app.login(&owner).await;
    let b = app.login(&owner).await;
    let page = a.get("/settings/sessions").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text().contains("this session"));

    let mismatch = a
        .post_form(
            "/settings/password",
            &[
                ("current_password", &owner.password),
                ("new_password", "brand-new-password"),
                ("confirm", "nope"),
            ],
        )
        .await;
    assert_eq!(mismatch.status, StatusCode::UNPROCESSABLE_ENTITY);
    let wrong = a
        .post_form(
            "/settings/password",
            &[
                ("current_password", "wrong-current-1"),
                ("new_password", "brand-new-password"),
                ("confirm", "brand-new-password"),
            ],
        )
        .await;
    assert_eq!(wrong.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(wrong.text().contains("current password is incorrect"));
    let ok = a
        .post_form(
            "/settings/password",
            &[
                ("current_password", &owner.password),
                ("new_password", "brand-new-password"),
                ("confirm", "brand-new-password"),
            ],
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert!(ok.text().contains("Password changed"));
    // The other session was revoked, this one survives.
    assert_eq!(a.get("/").await.status, StatusCode::OK);
    assert_eq!(b.get("/").await.status, StatusCode::SEE_OTHER);
}
