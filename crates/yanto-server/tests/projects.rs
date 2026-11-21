//! Projects & Deployments over HTTP: dashboard pages and forms, REST, webhooks, SSE.

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use yanto_core::actor::{Permission, Scope};
use yanto_core::deploy::worker;
use yanto_core::service::deployments::github_signature;
use yanto_core::testing::Reply;
use yanto_server::testing::{TestApp, TestClient};

async fn run_queue(app: &TestApp) {
    let eng = app.core.services.engine();
    while worker::run_once(eng.clone()).await {}
}

/// Create an image Project through the dashboard form; returns (project id, deploy token).
async fn create_project(app: &TestApp, c: &TestClient, name: &str) -> (String, String) {
    let res = c
        .post_form(
            "/projects",
            &[
                ("name", name),
                ("source", "nginx:alpine"),
                ("auto_start", "on"),
            ],
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text());
    let text = res.text();
    let token = text
        .split("ydp_")
        .nth(1)
        .expect("token on the created page");
    let token = format!("ydp_{}", token.split('<').next().unwrap().trim());
    let id: String = app
        .core
        .services
        .projects
        .list(&yanto_core::actor::Actor::cli())
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.name == name)
        .unwrap()
        .id;
    (id, token)
}

#[tokio::test]
async fn dashboard_create_shows_the_token_once_and_never_caches_it() {
    let app = TestApp::new().await;
    let c = app.owner_client().await;
    let res = c
        .post_form(
            "/projects",
            &[("name", "My App"), ("source", "nginx:alpine")],
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.header("cache-control"), Some("no-store"));
    let token = res
        .text()
        .split("ydp_")
        .nth(1)
        .map(|t| t.split('<').next().unwrap().to_owned())
        .unwrap();
    assert!(token.len() > 30);
    let id = app
        .core
        .services
        .projects
        .list(&yanto_core::actor::Actor::cli())
        .await
        .unwrap()[0]
        .id
        .clone();
    let page = c.get(&format!("/projects/{id}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        !page.text().contains(&token),
        "the detail page never prints the token"
    );
    let list = c.get("/projects").await.text();
    assert!(list.contains("My App") && list.contains("my-app"));
    let reveal = c
        .htmx(
            Method::POST,
            &format!("/projects/{id}/deploy-token/reveal"),
            &[],
        )
        .await;
    assert_eq!(reveal.status, StatusCode::OK);
    assert!(reveal.text().contains(&token));
    assert_eq!(reveal.header("cache-control"), Some("no-store"));
    // No inline scripts or styles anywhere (CSP is default-src 'self').
    for body in [page.text(), list] {
        assert!(
            !body.contains("<script>") && !body.contains("onclick") && !body.contains("style=")
        );
    }
}

#[tokio::test]
async fn dashboard_validation_errors_rerender_the_form() {
    let app = TestApp::new().await;
    let c = app.owner_client().await;
    let res = c
        .post_form(
            "/projects",
            &[
                ("name", "Bad"),
                ("source", "file:///etc/passwd"),
                ("branch", "master"),
            ],
        )
        .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(res.text().contains("role=\"alert\"") && res.text().contains("value=\"Bad\""));
    let missing_csrf = c
        .clone()
        .without_csrf()
        .post_form("/projects", &[("name", "x"), ("source", "nginx")])
        .await;
    assert_eq!(missing_csrf.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn deploy_from_the_dashboard_then_read_logs_over_http_and_sse() {
    let app = TestApp::new().await;
    let c = app.owner_client().await;
    let (id, _) = create_project(&app, &c, "Web").await;
    app.core.commands.on(
        "pull nginx:alpine",
        Reply {
            lines: vec![
                "<script>alert(1)</script>".into(),
                "\u{1b}[32mgreen\u{1b}[0m".into(),
            ],
            ..Reply::ok()
        },
    );
    let res = c
        .htmx(Method::POST, &format!("/projects/{id}/deploy"), &[])
        .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    let target = res.header("hx-redirect").unwrap().to_owned();
    let dep = target.rsplit('/').next().unwrap().to_owned();
    assert!(c.get(&target).await.text().contains("queued"));
    run_queue(&app).await;

    let page = c.get(&target).await.text();
    assert!(page.contains("success") && page.contains("data-stream"));
    let text = c.get(&format!("/deployments/{dep}/logs")).await;
    assert_eq!(
        text.header("content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert!(text.text().contains("Deployment succeeded"));

    let sse = c.get(&format!("/deployments/{dep}/logs/stream")).await;
    assert_eq!(sse.status, StatusCode::OK);
    assert!(
        sse.header("content-type")
            .unwrap()
            .starts_with("text/event-stream")
    );
    let body = sse.text();
    assert!(
        body.contains("event: log")
            && body.contains("event: done")
            && body.contains("data: success")
    );
    assert!(
        body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "log text is escaped: {body}"
    );
    assert!(!body.contains("<script>"));
    assert!(body.contains("<span class=\"ansi-32\">green</span>"));
    // Resume: Last-Event-ID skips what the client already has.
    let first_seq: i64 = body
        .lines()
        .find_map(|l| l.strip_prefix("id: "))
        .unwrap()
        .parse()
        .unwrap();
    let resumed = c
        .send(
            Method::GET,
            &format!("/deployments/{dep}/logs/stream"),
            None,
            &[("last-event-id", &first_seq.to_string())],
        )
        .await
        .text();
    assert!(!resumed.contains(&format!("id: {first_seq}\n")) && resumed.contains("event: done"));
    let list = c.get("/deployments").await.text();
    assert!(list.contains(&dep));
}

#[tokio::test]
async fn cancel_from_the_dashboard() {
    let app = TestApp::new().await;
    let c = app.owner_client().await;
    let (id, _) = create_project(&app, &c, "Cx").await;
    let res = c
        .htmx(Method::POST, &format!("/projects/{id}/deploy"), &[])
        .await;
    let dep = res
        .header("hx-redirect")
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let cancelled = c
        .htmx(Method::POST, &format!("/deployments/{dep}/cancel"), &[])
        .await;
    assert_eq!(cancelled.status, StatusCode::NO_CONTENT);
    assert!(
        c.get(&format!("/deployments/{dep}"))
            .await
            .text()
            .contains("cancelled")
    );
}

#[tokio::test]
async fn members_see_their_projects_only_and_buttons_follow_permissions() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    let (a, _) = create_project(&app, &owner, "Alpha").await;
    let (b, _) = create_project(&app, &owner, "Bravo").await;
    let m = app
        .member_client("dev", &[(&a, &[Permission::Deploy])])
        .await;
    let list = m.get("/projects").await.text();
    assert!(list.contains("Alpha") && !list.contains("Bravo"));
    let page = m.get(&format!("/projects/{a}")).await.text();
    assert!(
        page.contains("/deploy\"")
            && !page.contains("/deploy-token/reveal")
            && !page.contains("Delete project")
    );
    assert_eq!(
        m.get(&format!("/projects/{b}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        m.post_form(&format!("/projects/{b}/deploy"), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        m.htmx(Method::POST, &format!("/projects/{a}/stop"), &[])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(m.get("/projects/new").await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        m.htmx(
            Method::POST,
            &format!("/projects/{a}/deploy-token/reveal"),
            &[]
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn env_editor_masks_values_and_rollback_preview_renders() {
    let app = TestApp::new().await;
    let c = app.owner_client().await;
    let (id, _) = create_project(&app, &c, "Envy").await;
    let put = c
        .htmx(
            Method::PUT,
            &format!("/projects/{id}/env"),
            &[("content", "DB_PASSWORD=hunter2hunter2\nMODE=prod\n")],
        )
        .await;
    assert_eq!(put.status, StatusCode::OK, "{}", put.text());
    let page = c.get(&format!("/projects/{id}")).await.text();
    assert!(page.contains("DB_PASSWORD=********") && !page.contains("hunter2"));
    let preview = c
        .htmx(
            Method::POST,
            &format!("/projects/{id}/env/preview"),
            &[("content", "A=1\nbroken\n")],
        )
        .await
        .text();
    assert!(preview.contains("A") && preview.contains("line(s) 2"));
    // Two deployments, then a preview fragment.
    for _ in 0..2 {
        c.htmx(Method::POST, &format!("/projects/{id}/deploy"), &[])
            .await;
        run_queue(&app).await;
    }
    let prev = c
        .htmx(
            Method::POST,
            &format!("/projects/{id}/rollback/preview"),
            &[],
        )
        .await;
    assert_eq!(
        prev.status,
        StatusCode::CONFLICT,
        "same digest twice: nothing to roll back to"
    );
}

#[tokio::test]
async fn deploy_keys_and_credentials_pages_hide_secrets() {
    let app = TestApp::new().await;
    let c = app.owner_client().await;
    let res = c.post_form("/deploy-keys", &[("name", "ci")]).await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    let page = c.get("/deploy-keys").await.text();
    assert!(page.contains("ssh-ed25519 ") && !page.contains("OPENSSH PRIVATE KEY"));
    let res = c
        .post_form(
            "/registry-credentials",
            &[
                ("name", "ghcr"),
                ("registry_host", "ghcr.io"),
                ("username", "bot"),
                ("secret", "ghp_topsecretvalue"),
            ],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    let page = c.get("/registry-credentials").await.text();
    assert!(page.contains("ghcr.io") && !page.contains("ghp_topsecretvalue"));
    let dup = c
        .post_form(
            "/registry-credentials",
            &[
                ("name", "ghcr"),
                ("registry_host", "ghcr.io"),
                ("username", "bot"),
                ("secret", "x"),
            ],
        )
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    let nav = c.get("/").await.text();
    assert!(
        nav.contains("href=\"/projects\"")
            && nav.contains("href=\"/deployments\"")
            && nav.contains("href=\"/deploy-keys\"")
    );
}

// ---- REST ---------------------------------------------------------------------------------

async fn api(app: &TestApp, scope: Scope) -> TestClient {
    let owner = app.owner_user().await;
    app.client().with_bearer(&app.bearer(&owner, scope).await)
}

#[tokio::test]
async fn rest_project_lifecycle_and_secrets_handling() {
    let app = TestApp::new().await;
    let a = api(&app, Scope::Admin).await;
    let res = a
        .json(
            Method::POST,
            "/api/v1/projects",
            &json!({"name": "Rest", "image_ref": "nginx:alpine"}),
        )
        .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
    let v: Value = res.value();
    let id = v["project"]["id"].as_str().unwrap().to_owned();
    let token = v["deploy_token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("ydp_"));
    let got = a.get(&format!("/api/v1/projects/{id}")).await;
    assert!(!got.text().contains(&token) && !got.text().contains("deploy_token"));
    assert_eq!(
        a.json(Method::POST, "/api/v1/projects", &json!({"name": "x"}))
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        a.json(
            Method::POST,
            "/api/v1/projects",
            &json!({"name": "Rest", "image_ref": "nginx"})
        )
        .await
        .status,
        StatusCode::CONFLICT
    );

    // PATCH with explicit null clears; absent leaves alone.
    let p = a
        .json(
            Method::PATCH,
            &format!("/api/v1/projects/{id}"),
            &json!({"compose_content": "services: {}\n", "auto_start": false}),
        )
        .await;
    assert_eq!(p.value()["has_compose_content"], true);
    let p = a
        .json(
            Method::PATCH,
            &format!("/api/v1/projects/{id}"),
            &json!({"compose_content": null}),
        )
        .await;
    assert_eq!(p.value()["has_compose_content"], false);

    // Env: write, masked read, reveal needs admin.
    a.json(
        Method::PUT,
        &format!("/api/v1/projects/{id}/env"),
        &json!({"content": "TOKEN=abc12345\n"}),
    )
    .await;
    let masked = a.get(&format!("/api/v1/projects/{id}/env/content")).await;
    assert_eq!(masked.value()["content"], "TOKEN=********\n");
    let read_tok = api(&app, Scope::Read).await;
    assert_eq!(
        read_tok
            .get(&format!("/api/v1/projects/{id}/env/content?reveal=true"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        a.get(&format!("/api/v1/projects/{id}/env/content?reveal=true"))
            .await
            .value()["content"],
        "TOKEN=abc12345\n"
    );

    // Deploy: 202 {deployment, reused}; second trigger reuses the queued one.
    let w = api(&app, Scope::Write).await;
    let d = w
        .json(
            Method::POST,
            &format!("/api/v1/projects/{id}/deploy"),
            &json!({}),
        )
        .await;
    assert_eq!(d.status, StatusCode::ACCEPTED);
    assert_eq!(d.value()["reused"], false);
    assert_eq!(d.value()["deployment"]["trigger"], "api");
    let d2 = w
        .send(
            Method::POST,
            &format!("/api/v1/projects/{id}/deploy"),
            None,
            &[],
        )
        .await;
    assert_eq!(d2.value()["reused"], true);
    let dep = d.value()["deployment"]["id"].as_str().unwrap().to_owned();
    run_queue(&app).await;
    assert_eq!(
        a.get(&format!("/api/v1/deployments/{dep}")).await.value()["status"],
        "success"
    );
    assert!(
        a.get(&format!("/api/v1/deployments/{dep}/logs?tail=3"))
            .await
            .text()
            .lines()
            .count()
            <= 3
    );
    let sse = a
        .get(&format!("/api/v1/deployments/{dep}/logs/stream"))
        .await;
    assert!(sse.text().contains("event: done"));
    let list = a
        .get(&format!(
            "/api/v1/deployments?project_id={id}&status=success"
        ))
        .await;
    assert_eq!(list.value().as_array().unwrap().len(), 1);

    // Reveal / rotate are admin-only and equal what create returned.
    assert_eq!(
        w.json(
            Method::POST,
            &format!("/api/v1/projects/{id}/deploy-token/reveal"),
            &json!({})
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        a.json(
            Method::POST,
            &format!("/api/v1/projects/{id}/deploy-token/reveal"),
            &json!({})
        )
        .await
        .value()["deploy_token"],
        token
    );
    let rotated = a
        .json(
            Method::POST,
            &format!("/api/v1/projects/{id}/deploy-token/rotate"),
            &json!({}),
        )
        .await
        .value()["deploy_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(rotated, token);

    // Ports: loopback only.
    let bad = a.json(Method::PUT, &format!("/api/v1/projects/{id}/ports"), &json!({"ports": [{"label":"web","host_port":18080,"container_port":80,"bind_address":"0.0.0.0"}]})).await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let ok = a
        .json(
            Method::PUT,
            &format!("/api/v1/projects/{id}/ports"),
            &json!({"ports": [{"label":"web","host_port":18080,"container_port":80}]}),
        )
        .await;
    assert_eq!(ok.value()[0]["bind_address"], "127.0.0.1");

    // Delete needs admin; the project is gone afterwards.
    assert_eq!(
        w.send(Method::DELETE, &format!("/api/v1/projects/{id}"), None, &[])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        a.send(Method::DELETE, &format!("/api/v1/projects/{id}"), None, &[])
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        a.get(&format!("/api/v1/projects/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn rest_keys_and_credentials_never_return_private_material() {
    let app = TestApp::new().await;
    let a = api(&app, Scope::Admin).await;
    let k = a
        .json(Method::POST, "/api/v1/deploy-keys", &json!({"name": "k1"}))
        .await;
    assert_eq!(k.status, StatusCode::CREATED);
    assert!(
        k.value()["public_key"]
            .as_str()
            .unwrap()
            .starts_with("ssh-ed25519 ")
    );
    assert!(!k.text().contains("PRIVATE") && !k.text().contains("private_key"));
    let imported = a
        .json(
            Method::POST,
            "/api/v1/deploy-keys/import",
            &json!({"name": "k2", "private_key": "garbage"}),
        )
        .await;
    assert_eq!(imported.status, StatusCode::UNPROCESSABLE_ENTITY);
    let c = a.json(Method::POST, "/api/v1/registry-credentials", &json!({"name": "c", "registry_host": "ghcr.io", "username": "u", "secret": "s3cr3t-value"})).await;
    assert_eq!(c.status, StatusCode::CREATED);
    assert!(!c.text().contains("s3cr3t-value"));
    let list = a.get("/api/v1/registry-credentials").await.text();
    assert!(!list.contains("s3cr3t-value") && !list.contains("secret_enc"));
    let id = c.value()["id"].as_str().unwrap().to_owned();
    assert_eq!(
        a.json(
            Method::PATCH,
            &format!("/api/v1/registry-credentials/{id}"),
            &json!({"secret": "new-one"})
        )
        .await
        .status,
        StatusCode::OK
    );
}

// ---- webhooks -----------------------------------------------------------------------------

#[tokio::test]
async fn deploy_webhook_over_http() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    let (id, token) = create_project(&app, &owner, "Hooked").await;
    let anon = app.client();
    let post = |c: &TestClient, path: String, auth: Option<String>| {
        let c = c.clone();
        async move {
            let extra: Vec<(&str, String)> =
                auth.into_iter().map(|a| ("authorization", a)).collect();
            let refs: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
            c.send(
                Method::POST,
                &path,
                Some(("application/json", "{\"ref\":\"ignored\"}".into())),
                &refs,
            )
            .await
        }
    };
    let path = format!("/deploy?id={id}");
    assert_eq!(
        post(&anon, path.clone(), None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&anon, path.clone(), Some("Bearer ydp_wrong".into()))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(
            &anon,
            "/deploy?id=prj_unknown".into(),
            Some(format!("Bearer {token}"))
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&anon, "/deploy".into(), Some(format!("Bearer {token}")))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    let ok = post(&anon, path.clone(), Some(format!("Bearer {token}"))).await;
    assert_eq!(ok.status, StatusCode::ACCEPTED, "{}", ok.text());
    assert_eq!(ok.value()["deployment"]["trigger"], "deploy_token");
    assert_eq!(ok.value()["reused"], false);
    assert_eq!(
        post(&anon, path.clone(), Some(format!("Bearer {token}")))
            .await
            .value()["reused"],
        true
    );
    // A session cookie is not accepted on this route (and CSRF does not apply to it).
    let cookie_only = post(&owner.clone().without_csrf(), path.clone(), None).await;
    assert_eq!(cookie_only.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn github_webhook_over_http() {
    let app = TestApp::new().await;
    let a = api(&app, Scope::Admin).await;
    let created = a
        .json(Method::POST, "/api/v1/projects", &json!({"name": "Gh", "git_url": "https://github.com/o/gh.git", "compose_content": "services: {}\n"}))
        .await
        .value();
    let id = created["project"]["id"].as_str().unwrap().to_owned();
    let token = created["deploy_token"].as_str().unwrap().to_owned();
    let hook = app.client();
    let body = format!(
        r#"{{"ref":"refs/heads/master","after":"{}"}}"#,
        "d".repeat(40)
    );
    let send = |sig: Option<String>, event: &'static str, delivery: &'static str, body: String| {
        let hook = hook.clone();
        let id = id.clone();
        async move {
            let mut extra: Vec<(&str, String)> = vec![
                ("x-github-event", event.into()),
                ("x-github-delivery", delivery.into()),
            ];
            if let Some(s) = sig {
                extra.push(("x-hub-signature-256", s));
            }
            let refs: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
            hook.send(
                Method::POST,
                &format!("/webhooks/github?id={id}"),
                Some(("application/json", body)),
                &refs,
            )
            .await
        }
    };
    let sig = github_signature(&token, body.as_bytes());
    assert_eq!(
        send(None, "push", "a1", body.clone()).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            Some(github_signature("nope", body.as_bytes())),
            "push",
            "a1",
            body.clone()
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(Some(sig.clone()), "ping", "a2", body.clone())
            .await
            .status,
        StatusCode::OK
    );
    let ignored = send(Some(sig.clone()), "issues", "a3", body.clone()).await;
    assert_eq!(
        (ignored.status, ignored.value()["status"].clone()),
        (StatusCode::ACCEPTED, json!("ignored"))
    );
    let other = body.replace("master", "dev");
    assert_eq!(
        send(
            Some(github_signature(&token, other.as_bytes())),
            "push",
            "a4",
            other
        )
        .await
        .value()["reason"],
        "other branch"
    );
    let queued = send(Some(sig.clone()), "push", "a5", body.clone()).await;
    assert_eq!(queued.status, StatusCode::ACCEPTED, "{}", queued.text());
    assert_eq!(queued.value()["deployment"]["trigger"], "github");
    let dup = send(Some(sig.clone()), "push", "a5", body.clone()).await;
    assert_eq!(
        (dup.status, dup.value()["status"].clone()),
        (StatusCode::OK, json!("duplicate"))
    );
    // Oversized bodies are 413 before any parsing.
    let big = "x".repeat(1024 * 1024 + 10);
    assert_eq!(
        send(
            Some(github_signature(&token, big.as_bytes())),
            "push",
            "a6",
            big
        )
        .await
        .status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn failed_pull_offers_the_checkout_reset_in_the_dashboard_and_over_rest() {
    use yanto_core::service::projects::{ProjectInput, SourceInput};
    let app = TestApp::new().await;
    let cli = yanto_core::actor::Actor::cli();
    let p = app
        .core
        .services
        .projects
        .create(
            &cli,
            &ProjectInput {
                name: "gitty".into(),
                slug: Some("gitty".into()),
                source: SourceInput::Git {
                    url: "https://github.com/o/g.git".into(),
                    branch: None,
                },
                compose_path: None,
                compose_content: Some("services:\n  app:\n    image: nginx:alpine\n".into()),
                env_file: None,
                auto_start: None,
                manual_deploy_enabled: None,
                github_webhook_enabled: None,
                deploy_key_id: None,
                generate_deploy_key: false,
                registry_credential_id: None,
            },
        )
        .await
        .unwrap()
        .project;
    let deploy = || async {
        let d = app
            .core
            .services
            .deployments
            .deploy(&cli, &p.id, &Default::default())
            .await
            .unwrap()
            .deployment
            .id;
        run_queue(&app).await;
        d
    };
    deploy().await;
    std::fs::create_dir_all(app.core.dir.path().join("projects/gitty/.git")).unwrap();
    app.core.commands.on(
        "pull --ff-only",
        Reply::fail(128, "fatal: Not possible to fast-forward, aborting."),
    );
    let failed = deploy().await;

    let c = app.owner_client().await;
    let page = c.get(&format!("/deployments/{failed}")).await.text();
    assert!(page.contains("Reset checkout to origin"), "{page}");
    let done = c
        .htmx(
            Method::POST,
            &format!("/projects/{}/reset-checkout", p.id),
            &[],
        )
        .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text());
    assert!(done.text().contains("Checkout reset to origin/master"));

    // A Member with only view access sees neither the button nor the action.
    let m = app.member_client("mia", &[(&p.id, &[])]).await;
    assert!(
        !m.get(&format!("/deployments/{failed}"))
            .await
            .text()
            .contains("Reset checkout to origin")
    );
    assert_eq!(
        m.htmx(
            Method::POST,
            &format!("/projects/{}/reset-checkout", p.id),
            &[]
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );

    // REST: admin scope only.
    let owner = app.owner_user().await;
    let write = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Write).await);
    let admin = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Admin).await);
    let path = format!("/api/v1/projects/{}/reset-checkout", p.id);
    assert_eq!(
        write.json(Method::POST, &path, &json!({})).await.status,
        StatusCode::FORBIDDEN
    );
    let ok = admin.json(Method::POST, &path, &json!({})).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    assert_eq!(ok.value()["branch"], "master");
}
