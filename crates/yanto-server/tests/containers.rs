//! M3 adapters: container pages, REST, SSE, system pages and the exec WebSocket (against a real
//! listener, because a WebSocket upgrade cannot go through `oneshot`).

use std::time::Duration;

use axum::http::{Method, StatusCode};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use yanto_core::actor::{Permission, Scope};
use yanto_core::ports::{LogLine, LogStream, PruneOutcome, PrunePreviewRaw};
use yanto_server::ExecLimits;
use yanto_server::testing::TestApp;

#[tokio::test]
async fn container_pages_and_actions_for_the_owner() {
    let app = TestApp::new().await;
    app.core.docker.add_container(
        "shop-web-1",
        "running",
        &[
            ("com.docker.compose.project", "shop"),
            ("com.docker.compose.service", "web"),
        ],
    );
    app.core.docker.add_container("<img src=x>", "exited", &[]);
    let c = app.owner_client().await;

    let page = c.get("/containers").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.text();
    assert!(html.contains("shop-web-1") && html.contains("shop / web"));
    assert!(!html.contains("<img src=x>"), "names are escaped");
    assert!(html.contains("&lt;img src=x&gt;") || html.contains("&#60;img"));
    assert!(
        html.contains("Terminal"),
        "running containers link the terminal"
    );

    let rows = c.get("/containers/rows").await;
    assert!(rows.text().contains("shop-web-1") && !rows.text().contains("<html"));

    let detail = c.get("/containers/shop-web-1").await;
    assert_eq!(detail.status, StatusCode::OK);
    assert!(detail.text().contains("sleep 3600"));

    // htmx action: returns the refreshed rows and writes an audit row.
    let res = c
        .htmx(Method::POST, "/containers/shop-web-1/stop", &[])
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(
        res.text().contains("Start"),
        "row now offers Start: {}",
        res.text()
    );
    // Plain form action redirects.
    let res = c.post_form("/containers/shop-web-1/start", &[]).await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);

    // Missing container: htmx gets a toast, full page gets the error page.
    let res = c.htmx(Method::POST, "/containers/ghost/stop", &[]).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    assert_eq!(res.header("hx-retarget"), Some("#toast"));
}

#[tokio::test]
async fn docker_down_shows_a_banner_not_an_error_page() {
    let app = TestApp::new().await;
    let c = app.owner_client().await;
    app.core
        .docker
        .fail_next(yanto_core::ports::DockerError::Unavailable("sock".into()));
    let res = c.get("/containers").await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.text().contains("Docker unavailable"));
}

#[tokio::test]
async fn logs_page_escapes_and_sse_streams_json_lines() {
    let app = TestApp::new().await;
    app.core.docker.add_container("web", "running", &[]);
    app.core.docker.set_logs(vec![LogLine {
        stream: LogStream::Stdout,
        timestamp: Some("2026-01-01T00:00:00Z".into()),
        text: "<script>alert(1)</script>".into(),
    }]);
    app.core.docker.set_follow_logs(vec![LogLine {
        stream: LogStream::Stderr,
        timestamp: None,
        text: "live line".into(),
    }]);
    let c = app.owner_client().await;
    let page = c.get("/containers/web/logs").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(!page.text().contains("<script>alert(1)"));
    assert!(page.text().contains("&lt;script&gt;") || page.text().contains("&#60;script"));
    assert!(page.text().contains("/assets/js/container-logs.js"));

    let sse = c.get("/containers/web/logs/stream").await;
    assert_eq!(sse.status, StatusCode::OK);
    assert!(
        sse.header("content-type")
            .unwrap()
            .starts_with("text/event-stream")
    );
    let body = sse.text();
    assert!(body.contains("data: {") && body.contains("live line") && body.contains("stderr"));
}

#[tokio::test]
async fn rest_containers_json_logs_and_no_exec_endpoint() {
    let app = TestApp::new().await;
    app.core.docker.add_container("web", "running", &[]);
    app.core.docker.set_logs(vec![LogLine {
        stream: LogStream::Stdout,
        timestamp: Some("2026-01-01T00:00:00Z".into()),
        text: "hello".into(),
    }]);
    let owner = app.owner_user().await;
    let admin = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Admin).await);

    let list = admin.get("/api/v1/containers").await;
    assert_eq!(list.status, StatusCode::OK);
    let v = list.value();
    assert_eq!(v["containers"][0]["name"], "web");
    assert!(v["containers"][0]["project"].is_null());

    let one = admin.get("/api/v1/containers/web").await.value();
    assert_eq!(one["name"], "web");
    assert!(one["detail"]["mounts"].is_array());

    let logs = admin.get("/api/v1/containers/web/logs?tail=10").await;
    assert_eq!(
        logs.header("content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert!(logs.text().contains("[stdout] hello"));

    let stop = admin
        .json(Method::POST, "/api/v1/containers/web/stop", &json!({}))
        .await;
    assert_eq!(stop.status, StatusCode::NO_CONTENT);
    assert!(admin.get("/api/v1/containers/ghost").await.status == StatusCode::NOT_FOUND);

    // D24: no REST exec, whatever the verb or scope.
    for m in [Method::GET, Method::POST, Method::PUT] {
        let r = admin
            .json(m, "/api/v1/containers/web/exec", &json!({"cmd": ["sh"]}))
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn system_page_cleanup_flow() {
    let app = TestApp::new().await;
    app.core.docker.set_preview(PrunePreviewRaw {
        build_cache: vec![yanto_core::testing::docker::FakeDocker::candidate(
            "bc",
            "cache layer",
            Some(2048),
        )],
        containers: vec![yanto_core::testing::docker::FakeDocker::candidate(
            "c1", "old-job", None,
        )],
        ..Default::default()
    });
    app.core.docker.set_prune_outcome(PruneOutcome {
        deleted: 1,
        reclaimed_bytes: 2048,
    });
    let c = app.owner_client().await;
    let page = c.get("/system").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.text();
    assert!(html.contains("Health") && html.contains("cache layer") && html.contains("old-job"));
    assert!(html.contains("prune containers"));
    assert!(!html.to_lowercase().contains("volumes are pruned"));

    // Containers selected without the phrase: rejected, nothing pruned.
    let bad = c
        .post_form(
            "/system/cleanup",
            &[("containers", "on"), ("confirm", "no")],
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        app.core
            .docker
            .calls()
            .iter()
            .all(|x| !x.starts_with("prune "))
    );

    let ok = c
        .post_form(
            "/system/cleanup",
            &[("build_cache", "on"), ("images", "on")],
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert!(ok.text().contains("Reclaimed") && ok.text().contains("Build cache"));

    // Members never see System.
    let m = app
        .member_client("m", &[("prj_1", &[Permission::Runtime])])
        .await;
    assert_eq!(m.get("/system").await.status, StatusCode::FORBIDDEN);
    assert!(!m.get("/").await.text().contains("href=\"/system\""));
}

#[tokio::test]
async fn rest_system_endpoints() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let read = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Read).await);
    let admin = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Admin).await);

    let h = read.get("/api/v1/health").await.value();
    assert_eq!(h["status"], "ok");
    assert_eq!(h["checks"].as_array().unwrap().len(), 3);
    let u = read.get("/api/v1/system/usage").await.value();
    assert_eq!(u["host"]["cpu_count"], 4);
    let p = read.get("/api/v1/system/cleanup/preview").await.value();
    assert_eq!(p["groups"].as_array().unwrap().len(), 4);
    assert_eq!(
        read.json(Method::POST, "/api/v1/system/cleanup", &json!({}))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let r = admin
        .json(Method::POST, "/api/v1/system/cleanup", &json!({}))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.value()["steps"].as_array().unwrap().len(), 3);
    let bad = admin
        .json(
            Method::POST,
            "/api/v1/system/cleanup",
            &json!({"containers": true}),
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn terminal_and_log_assets_are_served_and_pages_have_no_inline_script() {
    let app = TestApp::new().await;
    app.core.docker.add_container("web", "running", &[]);
    let c = app.owner_client().await;
    for path in [
        "/assets/vendor/xterm.js",
        "/assets/vendor/xterm.css",
        "/assets/vendor/xterm-addon-fit.js",
        "/assets/js/exec-terminal.js",
        "/assets/js/container-logs.js",
    ] {
        assert_eq!(c.get(path).await.status, StatusCode::OK, "{path}");
    }
    let page = c.get("/containers/web/exec").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.text();
    assert!(html.contains("/assets/vendor/xterm.js") && html.contains("data-ws-path"));
    assert!(!html.contains("<script>"), "CSP forbids inline scripts");
    let csp = page.header("content-security-policy").unwrap();
    assert!(csp.contains("style-src 'self' 'unsafe-inline'") && csp.contains("connect-src 'self'"));
    assert!(
        !csp.contains("script-src") && !csp.contains("'unsafe-eval'"),
        "scripts stay default-src 'self'"
    );
    let other = c.get("/containers").await;
    assert!(
        !other
            .header("content-security-policy")
            .unwrap()
            .contains("unsafe-inline")
    );
    // A stopped container is refused before any WebSocket.
    app.core.docker.add_container("idle", "exited", &[]);
    assert_eq!(
        c.get("/containers/idle/exec").await.status,
        StatusCode::CONFLICT
    );
}

// ---- exec WebSocket over a real listener ----------------------------------------------------

struct Live {
    app: TestApp,
    addr: std::net::SocketAddr,
}

async fn live(limits: Option<ExecLimits>) -> Live {
    let app = TestApp::new().await;
    app.core.docker.add_container("web", "running", &[]);
    let mut state = app.state.clone();
    if let Some(l) = limits {
        state.exec_limits = l;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(yanto_server::serve(listener, state, std::future::pending()));
    Live { app, addr }
}

fn ws_request(
    addr: std::net::SocketAddr,
    cookie: Option<&str>,
    origin: Option<&str>,
    query: &str,
) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut req = format!("ws://{addr}/containers/web/exec/ws{query}")
        .into_client_request()
        .unwrap();
    if let Some(c) = cookie {
        req.headers_mut()
            .insert("cookie", format!("yanto_session={c}").parse().unwrap());
    }
    if let Some(o) = origin {
        req.headers_mut().insert("origin", o.parse().unwrap());
    }
    req
}

fn http_status(e: &WsError) -> Option<u16> {
    match e {
        WsError::Http(r) => Some(r.status().as_u16()),
        _ => None,
    }
}

async fn eventually(what: &str, mut f: impl AsyncFnMut() -> bool) {
    for _ in 0..100 {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn audit_n(app: &TestApp, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE action = ?")
        .bind(action)
        .fetch_one(app.core.db.reader())
        .await
        .unwrap()
}

#[tokio::test]
async fn ws_pumps_bytes_resizes_and_audits_start_and_end() {
    let l = live(None).await;
    l.app
        .core
        .docker
        .set_exec_output(vec![b"hello\r\n".to_vec()]);
    let cookie = l.app.owner_client().await.cookie.unwrap();
    let origin = format!("http://{}", l.addr);
    let (mut ws, _) = connect_async(ws_request(
        l.addr,
        Some(&cookie),
        Some(&origin),
        "?cols=100&rows=30",
    ))
    .await
    .expect("handshake");

    // Output -> binary frame.
    let first = ws.next().await.unwrap().unwrap();
    assert_eq!(first.into_data().as_ref(), b"hello\r\n");
    assert!(
        l.app
            .core
            .docker
            .calls()
            .iter()
            .any(|c| c.ends_with("100x30"))
    );
    assert_eq!(audit_n(&l.app, "container.exec_start").await, 1);

    // Typed input -> exec stdin; resize -> Engine resize.
    ws.send(Message::Binary(b"ls\n".to_vec().into()))
        .await
        .unwrap();
    ws.send(Message::text(r#"{"type":"resize","cols":120,"rows":40}"#))
        .await
        .unwrap();
    eventually("input and resize", async || {
        l.app.core.docker.exec_input() == b"ls\n" && l.app.core.docker.resizes() == [(120, 40)]
    })
    .await;

    ws.close(None).await.unwrap();
    eventually("end audit", async || {
        audit_n(&l.app, "container.exec_end").await == 1
    })
    .await;
    assert!(l.app.core.docker.exec_input_closed());
}

#[tokio::test]
async fn ws_refuses_bad_origin_missing_session_no_permission_and_stopped() {
    let l = live(None).await;
    let owner = l.app.owner_client().await;
    let cookie = owner.cookie.clone().unwrap();
    let good_origin = format!("http://{}", l.addr);

    let err = connect_async(ws_request(
        l.addr,
        Some(&cookie),
        Some("http://evil.example"),
        "",
    ))
    .await
    .unwrap_err();
    assert_eq!(http_status(&err), Some(403), "cross-site origin");
    let err = connect_async(ws_request(l.addr, Some(&cookie), None, ""))
        .await
        .unwrap_err();
    assert_eq!(http_status(&err), Some(403), "no origin");
    let err = connect_async(ws_request(l.addr, None, Some(&good_origin), ""))
        .await
        .unwrap_err();
    assert!(
        matches!(http_status(&err), Some(303 | 401)),
        "no session: {err:?}"
    );

    // Member without a visible container: 404; and nothing was ever exec'd.
    let member = l
        .app
        .member_client("m", &[("prj_1", &[Permission::Runtime])])
        .await;
    let err = connect_async(ws_request(
        l.addr,
        member.cookie.as_deref(),
        Some(&good_origin),
        "",
    ))
    .await
    .unwrap_err();
    assert_eq!(http_status(&err), Some(404));
    assert!(
        l.app
            .core
            .docker
            .calls()
            .iter()
            .all(|c| !c.starts_with("exec_start"))
    );

    // Stopped container: 409 before the upgrade.
    l.app.core.docker.add_container("web2", "exited", &[]);
    let mut req = ws_request(l.addr, Some(&cookie), Some(&good_origin), "");
    *req.uri_mut() = format!("ws://{}/containers/web2/exec/ws", l.addr)
        .parse()
        .unwrap();
    let err = connect_async(req).await.unwrap_err();
    assert_eq!(http_status(&err), Some(409));
    assert_eq!(audit_n(&l.app, "container.exec_start").await, 0);
}

#[tokio::test]
async fn ws_idle_timeout_closes_the_session() {
    let l = live(Some(ExecLimits {
        idle: Duration::from_millis(200),
        max_session: Duration::from_secs(60),
    }))
    .await;
    let cookie = l.app.owner_client().await.cookie.unwrap();
    let origin = format!("http://{}", l.addr);
    let (mut ws, _) = connect_async(ws_request(l.addr, Some(&cookie), Some(&origin), ""))
        .await
        .unwrap();
    let close = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(f))) => return f,
                Some(Ok(_)) => {}
                _ => return None,
            }
        }
    })
    .await
    .expect("closed by idle timeout");
    assert_eq!(close.unwrap().reason.as_str(), "idle timeout");
    eventually("end audit", async || {
        audit_n(&l.app, "container.exec_end").await == 1
    })
    .await;
    let meta: String =
        sqlx::query_scalar("SELECT metadata FROM audit_logs WHERE action = 'container.exec_end'")
            .fetch_one(l.app.core.db.reader())
            .await
            .unwrap();
    assert!(meta.contains("\"reason\":\"idle\""), "{meta}");
}

#[tokio::test]
async fn ws_rejects_oversized_frames() {
    let l = live(None).await;
    let cookie = l.app.owner_client().await.cookie.unwrap();
    let origin = format!("http://{}", l.addr);
    let (mut ws, _) = connect_async(ws_request(l.addr, Some(&cookie), Some(&origin), ""))
        .await
        .unwrap();
    ws.send(Message::Binary(vec![b'a'; 100 * 1024].into()))
        .await
        .unwrap();
    // The server drops the connection; the shell never sees the data.
    let ended = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "connection closed");
    assert!(l.app.core.docker.exec_input().len() < 100 * 1024);
}
