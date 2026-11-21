//! HTTP test harness (feature `testing`; always on for this crate's own tests).
//!
//! ```ignore
//! let app = TestApp::new().await;                       // temp SQLite, fake clock, no Owner yet
//! let owner = app.owner_client().await;                 // Owner signed in through POST /login
//! let alice = app.member_client("alice", &[("prj_1", &[Permission::Deploy])]).await;
//! let token = app.bearer(&app.owner_user().await, Scope::Admin).await;
//! let res = app.client().with_bearer(&token).get("/api/v1/me").await;
//! assert_eq!(res.status, 200);
//! ```
//!
//! Requests go straight into the router (`tower::ServiceExt::oneshot`), with a fake peer
//! address so rate limits and audit IPs are testable. Unsafe dashboard requests automatically
//! carry `Origin`, `Host` and (for signed-in clients) the CSRF header; use
//! [`TestClient::without_csrf`] / [`TestClient::with_origin`] to test the failure paths.

use std::net::{IpAddr, SocketAddr};

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use tower::ServiceExt;
use yanto_core::actor::{Permission, Scope};
use yanto_core::testing::{TestCore, TestUser};

use crate::{AppState, router};

pub const HOST: &str = "localhost:9197";

/// A throwaway app: temp DB, fake clock, real services, real router.
pub struct TestApp {
    pub core: TestCore,
    pub state: AppState,
    router: Router,
}

impl TestApp {
    pub async fn new() -> Self {
        let core = TestCore::new().await;
        let state = AppState::new(core.db.clone(), core.config.clone(), core.services.clone());
        let router = router(state.clone());
        Self {
            core,
            state,
            router,
        }
    }

    /// Like [`TestApp::new`] but with `server.public_url` set (cookie `Secure`, origin allow-list).
    pub async fn with_public_url(url: &str) -> Self {
        let mut app = Self::new().await;
        let mut config = (*app.state.config).clone();
        config.server.public_url = url.to_owned();
        app.state.config = std::sync::Arc::new(config);
        app.router = router(app.state.clone());
        app
    }

    /// An anonymous client (no cookie, no token).
    pub fn client(&self) -> TestClient {
        TestClient::new(self.router.clone())
    }

    /// Create the Owner (through the real setup flow) if needed and return its fixture.
    pub async fn owner_user(&self) -> TestUser {
        if self
            .core
            .services
            .auth
            .setup_pending()
            .await
            .expect("setup state")
        {
            self.core.setup_owner().await
        } else {
            TestUser {
                id: String::new(),
                username: "owner".into(),
                password: yanto_core::testing::OWNER_PASSWORD.into(),
            }
        }
    }

    /// Sign the Owner in through `POST /login` and return a client holding the session.
    pub async fn owner_client(&self) -> TestClient {
        let owner = self.owner_user().await;
        self.login(&owner).await
    }

    /// Create a Member through the invite flow and sign them in.
    pub async fn member_client(
        &self,
        username: &str,
        grants: &[(&str, &[Permission])],
    ) -> TestClient {
        self.owner_user().await;
        let user = self.core.create_member(username, grants).await;
        self.login(&user).await
    }

    /// Sign in with the real login form and read the CSRF token from the rendered page.
    pub async fn login(&self, user: &TestUser) -> TestClient {
        let mut c = self.client();
        let res = c
            .post_form(
                "/login",
                &[("username", &user.username), ("password", &user.password)],
            )
            .await;
        assert_eq!(
            res.status,
            StatusCode::SEE_OTHER,
            "login failed: {}",
            res.text()
        );
        c.cookie = Some(res.session_cookie().expect("session cookie after login"));
        let home = c.get("/").await;
        assert_eq!(
            home.status,
            StatusCode::OK,
            "home after login: {}",
            home.text()
        );
        c.csrf = Some(home.csrf_token().expect("csrf token in layout"));
        c
    }

    /// A bearer API Token for `user` with `scope` (created through the service, as the dashboard does).
    pub async fn bearer(&self, user: &TestUser, scope: Scope) -> String {
        self.core.token_actor(user, scope).await.0
    }
}

#[derive(Clone)]
pub struct TestClient {
    router: Router,
    pub cookie: Option<String>,
    pub csrf: Option<String>,
    bearer: Option<String>,
    ip: IpAddr,
    send_csrf: bool,
    origin: Option<String>,
}

impl TestClient {
    fn new(router: Router) -> Self {
        Self {
            router,
            cookie: None,
            csrf: None,
            bearer: None,
            ip: "203.0.113.7".parse().expect("ip"),
            send_csrf: true,
            origin: Some(format!("http://{HOST}")),
        }
    }

    pub fn with_bearer(mut self, token: &str) -> Self {
        self.bearer = Some(token.to_owned());
        self
    }

    pub fn with_ip(mut self, ip: &str) -> Self {
        self.ip = ip.parse().expect("ip");
        self
    }

    /// Do not send the CSRF header/field automatically.
    pub fn without_csrf(mut self) -> Self {
        self.send_csrf = false;
        self
    }

    /// Override the `Origin` header (`None` omits it).
    pub fn with_origin(mut self, origin: Option<&str>) -> Self {
        self.origin = origin.map(str::to_owned);
        self
    }

    pub async fn get(&self, path: &str) -> TestResponse {
        self.send(Method::GET, path, None, &[]).await
    }

    /// `application/x-www-form-urlencoded` POST. Adds `_csrf` for signed-in clients.
    pub async fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> TestResponse {
        self.form(Method::POST, path, fields).await
    }

    pub async fn form(&self, method: Method, path: &str, fields: &[(&str, &str)]) -> TestResponse {
        let mut ser = form_urlencoded::Serializer::new(String::new());
        for (k, v) in fields {
            ser.append_pair(k, v);
        }
        if self.send_csrf
            && let Some(t) = &self.csrf
        {
            ser.append_pair("_csrf", t);
        }
        self.send(
            method,
            path,
            Some(("application/x-www-form-urlencoded", ser.finish())),
            &[],
        )
        .await
    }

    /// htmx-style request: `HX-Request: true` and the CSRF header (as `hx-headers` would send).
    pub async fn htmx(&self, method: Method, path: &str, fields: &[(&str, &str)]) -> TestResponse {
        let mut ser = form_urlencoded::Serializer::new(String::new());
        for (k, v) in fields {
            ser.append_pair(k, v);
        }
        let body = (!matches!(method, Method::GET))
            .then(|| ("application/x-www-form-urlencoded", ser.finish()));
        self.send(method, path, body, &[("hx-request", "true")])
            .await
    }

    pub async fn json(&self, method: Method, path: &str, body: &serde_json::Value) -> TestResponse {
        self.send(
            method,
            path,
            Some(("application/json", body.to_string())),
            &[],
        )
        .await
    }

    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<(&str, String)>,
        extra: &[(&str, &str)],
    ) -> TestResponse {
        let mut req = Request::builder()
            .method(method.clone())
            .uri(path)
            .header(header::HOST, HOST);
        let unsafe_method = !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
        if unsafe_method && let Some(o) = &self.origin {
            req = req.header(header::ORIGIN, o);
        }
        if let Some(c) = &self.cookie {
            req = req.header(header::COOKIE, format!("yanto_session={c}"));
        }
        if let Some(t) = &self.bearer {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        if self.send_csrf
            && let Some(t) = &self.csrf
        {
            req = req.header("x-csrf-token", t);
        }
        for (k, v) in extra {
            req = req.header(*k, *v);
        }
        let body = match body {
            Some((ct, b)) => {
                req = req.header(header::CONTENT_TYPE, ct);
                Body::from(b)
            }
            None => Body::empty(),
        };
        let mut req = req.body(body).expect("request");
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(self.ip, 40_000)));
        let res = self
            .router
            .clone()
            .oneshot(req)
            .await
            .expect("router is infallible");
        let (parts, body) = res.into_parts();
        let bytes = body.collect().await.expect("body").to_bytes();
        TestResponse {
            status: parts.status,
            headers: parts.headers,
            body: bytes.to_vec(),
        }
    }
}

#[derive(Debug)]
pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl TestResponse {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("body is not the expected JSON ({e}): {}", self.text()))
    }

    pub fn value(&self) -> serde_json::Value {
        self.json()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn location(&self) -> Option<&str> {
        self.header("location")
    }

    /// The `yanto_session` value from `Set-Cookie`, if any (empty after logout).
    pub fn session_cookie(&self) -> Option<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|c| c.strip_prefix("yanto_session="))
            .map(|rest| rest.split(';').next().unwrap_or("").to_owned())
    }

    /// Raw `Set-Cookie` header for the session cookie.
    pub fn set_cookie_header(&self) -> Option<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|c| c.starts_with("yanto_session="))
            .map(str::to_owned)
    }

    /// CSRF token rendered into `<body hx-headers=...>`.
    pub fn csrf_token(&self) -> Option<String> {
        let text = self.text();
        let marker = "X-CSRF-Token\": \"";
        let start = text.find(marker)? + marker.len();
        let end = text[start..].find('"')? + start;
        Some(text[start..end].to_owned())
    }
}
