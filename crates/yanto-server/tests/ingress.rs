//! Hostnames, Cloudflare, Traefik, FRP and Host Daemons over REST and the dashboard.

use axum::http::{Method, StatusCode};
use serde_json::json;
use yanto_core::actor::{Permission, Scope};
use yanto_core::testing::ingress::CF_TOKEN;
use yanto_server::testing::{TestApp, TestClient};

async fn admin(app: &TestApp) -> TestClient {
    let owner = app.owner_user().await;
    app.client()
        .with_bearer(&app.bearer(&owner, Scope::Admin).await)
}

fn no_inline_script(html: &str) {
    assert!(
        !html.contains("onclick") && !html.contains("<script>"),
        "inline script in page"
    );
}

#[tokio::test]
async fn rest_cloudflare_tunnel_and_hostname_end_to_end() {
    let app = TestApp::new().await;
    let c = admin(&app).await;
    app.core.ingress.cloudflare.accept_token(CF_TOKEN);
    app.core
        .ingress
        .cloudflare
        .add_zone("zone-1", "example.com");
    app.core
        .ingress
        .systemd
        .install("cloudflared", true, "/usr/bin/cloudflared tunnel run");
    app.core.ensure_project("prj_1").await;
    let port = app.core.ensure_port("prj_1", 18080).await;

    // A wrong token is refused with the API's message and without echoing it.
    let bad = c
        .json(
            Method::POST,
            "/api/v1/cloudflare/clients",
            &json!({"name": "c", "account_id": "0123456789abcdef0123456789abcdef", "api_token": "wrong-secret"}),
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!bad.text().contains("wrong-secret"));

    let res = c
        .json(
            Method::POST,
            "/api/v1/cloudflare/clients",
            &json!({"name": "main", "account_id": "0123456789abcdef0123456789abcdef", "api_token": CF_TOKEN}),
        )
        .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
    assert!(!res.text().contains(CF_TOKEN));
    let client_id = res.value()["id"].as_str().unwrap().to_owned();
    let list = c.get("/api/v1/cloudflare/clients").await;
    assert!(!list.text().contains(CF_TOKEN));
    let zones = c
        .get(&format!(
            "/api/v1/cloudflare/clients/{client_id}/zones?refresh=true"
        ))
        .await
        .value();
    assert_eq!(zones[0]["name"], "example.com");

    let t = c
        .json(
            Method::POST,
            "/api/v1/cloudflare/tunnels",
            &json!({"client_id": client_id, "name": "vps"}),
        )
        .await;
    assert_eq!(t.status, StatusCode::CREATED, "{}", t.text());
    assert!(!t.text().contains("fake-connector-token"));
    let tunnel_id = t.value()["tunnel"]["id"].as_str().unwrap().to_owned();

    // The connector token is not reachable over REST at all.
    let r = c
        .json(
            Method::POST,
            &format!("/api/v1/cloudflare/tunnels/{tunnel_id}/token/reveal"),
            &json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    let h = c
        .json(
            Method::POST,
            "/api/v1/hostnames",
            &json!({"project_id": "prj_1", "project_port_id": port, "hostname": "App.Example.com",
                "ingress_provider": "cloudflare_tunnel", "zone_id": "zone-1", "manage_dns": true}),
        )
        .await;
    assert_eq!(h.status, StatusCode::CREATED, "{}", h.text());
    assert_eq!(h.value()["hostname"]["sync_status"], "active");
    assert_eq!(h.value()["hostname"]["hostname"], "app.example.com");
    let id = h.value()["hostname"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        c.get("/api/v1/hostnames")
            .await
            .value()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let d = c
        .get(&format!("/api/v1/hostnames/diagnostics?id={id}"))
        .await
        .value();
    assert!(d[0]["checks"].as_array().unwrap().len() >= 5);
    let del = c
        .json(
            Method::DELETE,
            &format!("/api/v1/hostnames/{id}"),
            &json!({}),
        )
        .await;
    assert_eq!(del.status, StatusCode::OK);
    let st = c
        .get(&format!("/api/v1/cloudflare/tunnels/{tunnel_id}/status"))
        .await;
    assert_eq!(st.status, StatusCode::OK);
}

#[tokio::test]
async fn dashboard_reveal_is_audited_no_store_and_post_only() {
    let app = TestApp::new().await;
    let f = app.core.setup_cloudflare().await;
    let owner = app.owner_client().await;
    let page = owner.get("/cloudflare").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(!page.text().contains("fake-connector-token"));
    no_inline_script(&page.text());
    // GET must not reveal (no link can leak it).
    let get = owner
        .get(&format!("/cloudflare/tunnels/{}/token/reveal", f.tunnel_id))
        .await;
    assert_eq!(get.status, StatusCode::METHOD_NOT_ALLOWED);
    let res = owner
        .post_form(
            &format!("/cloudflare/tunnels/{}/token/reveal", f.tunnel_id),
            &[],
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.header("cache-control"), Some("no-store"));
    let html = res.text();
    assert!(html.contains(&format!(
        "cloudflared service install fake-connector-token-{}",
        f.cf_tunnel_id
    )));
    assert!(html.contains("Yanto does not run it"));
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE action = 'cloudflare_tunnel.token_reveal'",
    )
    .fetch_one(app.core.db.reader())
    .await
    .unwrap();
    assert_eq!(n, 1);
    // CSRF applies: the same POST without the token is refused.
    let no_csrf = app
        .owner_client()
        .await
        .without_csrf()
        .post_form(
            &format!("/cloudflare/tunnels/{}/token/reveal", f.tunnel_id),
            &[],
        )
        .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn daemons_and_frps_over_rest() {
    let app = TestApp::new().await;
    let c = admin(&app).await;
    let d = c.get("/api/v1/daemons").await.value();
    assert_eq!(d.as_array().unwrap().len(), 3);
    assert_eq!(d[2]["name"], "frps");
    assert_eq!(d[2]["installed"], false);
    assert!(!d[2]["instructions"].as_array().unwrap().is_empty());
    let no_confirm = c
        .json(Method::POST, "/api/v1/daemons/frps/restart", &json!({}))
        .await;
    assert_eq!(no_confirm.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(no_confirm.text().contains("drops every live frp tunnel"));
    let missing = c
        .json(
            Method::POST,
            "/api/v1/daemons/frps/restart",
            &json!({"confirm": true}),
        )
        .await;
    assert_eq!(missing.status, StatusCode::CONFLICT, "never installs");

    let path = app.core.config.frp.config_path.clone();
    app.core.ingress.systemd.install(
        "frps",
        true,
        &format!("/usr/local/bin/frps -c {}", path.display()),
    );
    app.core
        .ingress
        .frp_files
        .put(&path, "bindPort = 7000\n# keep me\n", 0o600);
    let o = c.get("/api/v1/frp").await.value();
    assert_eq!(o["status"], "running_no_dashboard");
    let s = c
        .json(
            Method::PATCH,
            "/api/v1/frp/settings",
            &json!({"bind_port": 7001, "allow_ports": [{"start": 20000, "end": 20010}, {"single": 20020}]}),
        )
        .await;
    assert_eq!(s.status, StatusCode::OK, "{}", s.text());
    assert_eq!(s.value()["pending_restart"], true);
    let text = app.core.ingress.frp_files.content(&path).unwrap();
    assert!(text.contains("bindPort = 7001") && text.contains("# keep me"));
    let bad = c
        .json(
            Method::PATCH,
            "/api/v1/frp/settings",
            &json!({"allow_ports": [{"start": 9, "end": 3}]}),
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(app.core.ingress.frp_files.content(&path).unwrap(), text);
    let ok = c
        .json(
            Method::POST,
            "/api/v1/daemons/frps/restart",
            &json!({"confirm": true}),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    assert_eq!(c.get("/api/v1/frp").await.value()["pending_restart"], false);
    // The token reveal is not on REST.
    let r = c
        .json(Method::POST, "/api/v1/frp/token/reveal", &json!({}))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn owner_pages_render_and_warn_before_dropping_tunnels() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    for p in [
        "/hostnames",
        "/traefik",
        "/cloudflare",
        "/dns",
        "/frp",
        "/daemons",
        "/hostnames/diagnostics",
    ] {
        let r = owner.get(p).await;
        assert_eq!(r.status, StatusCode::OK, "{p}");
        no_inline_script(&r.text());
    }
    let frp = owner.get("/frp").await.text();
    assert!(
        frp.contains("not installed") && frp.contains("[Unit]"),
        "install help and sample unit"
    );
    let traefik = owner.get("/traefik").await.text();
    assert!(
        traefik.contains("certificatesResolvers"),
        "static prerequisites are shown"
    );

    // Installed frps with a pending config: the restart asks for confirmation with the warning.
    let path = app.core.config.frp.config_path.clone();
    app.core.ingress.systemd.install(
        "frps",
        true,
        &format!("/usr/local/bin/frps -c {}", path.display()),
    );
    app.core
        .ingress
        .frp_files
        .put(&path, "bindPort = 7000\n", 0o600);
    let r = owner
        .htmx(
            Method::PUT,
            "/frp/settings",
            &[
                ("bind_port", "7002"),
                ("bind_addr", ""),
                ("allow_ports", "3000-3010"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let frp = owner.get("/frp").await.text();
    assert!(frp.contains("hx-confirm=\"Restart frps now? This drops every live tunnel.\""));
    assert!(frp.contains("not applied yet"));
    assert!(!frp.contains("hunter2"));
    let nav = owner.get("/").await.text();
    for l in ["/hostnames", "/cloudflare", "/dns", "/frp", "/daemons"] {
        assert!(nav.contains(&format!("href=\"{l}\"")), "nav has {l}");
    }
}

#[tokio::test]
async fn member_with_hostnames_permission_uses_the_hostnames_page_only() {
    let app = TestApp::new().await;
    app.core.enable_traefik();
    let port = app.core.ensure_port("prj_1", 18080).await;
    let m = app
        .member_client("mia", &[("prj_1", &[Permission::Hostnames])])
        .await;
    let page = m.get("/hostnames").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text().contains("127.0.0.1:18080"));
    assert!(
        !page.text().contains("Re-sync providers"),
        "owner tools are hidden"
    );
    assert!(m.get("/").await.text().contains("href=\"/hostnames\""));
    for p in ["/cloudflare", "/dns", "/frp", "/daemons", "/traefik"] {
        assert_eq!(m.get(p).await.status, StatusCode::FORBIDDEN, "{p}");
    }
    let created = m
        .post_form(
            "/hostnames",
            &[
                ("target", &format!("prj_1|{port}")),
                ("hostname", "mia.example.com"),
                ("provider", "traefik"),
            ],
        )
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    assert!(created.text().contains("mia.example.com"));
    assert!(
        app.core
            .ingress
            .traefik
            .current()
            .unwrap()
            .contains("mia.example.com")
    );
    // Invalid input re-renders the form with the message and keeps what was typed.
    let bad = m
        .post_form(
            "/hostnames",
            &[
                ("target", &format!("prj_1|{port}")),
                ("hostname", "bad host`.example.com"),
                ("provider", "traefik"),
            ],
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(bad.text().contains("hostname"));
    let frag = m.htmx(Method::GET, "/projects/prj_1/hostnames", &[]).await;
    assert!(frag.text().contains("mia.example.com"));
}
