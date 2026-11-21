//! Cron Entries, Authorized Keys and Backups over REST and the dashboard.

use axum::http::{Method, StatusCode};
use serde_json::json;
use yanto_core::actor::{Permission, Scope};
use yanto_core::backup;
use yanto_server::testing::{TestApp, TestClient};

const KEY_A: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB laptop";
const KEY_B: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgIC ci";

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
async fn cron_over_rest_end_to_end() {
    let app = TestApp::new().await;
    let c = admin(&app).await;
    let body = json!({"name": "nightly", "schedule": "0 3 * * *", "run_as_user": "deploy",
        "command": "yanto backup yanto --keep-last 14", "overlap": "skip", "timeout_s": 600});
    let res = c.json(Method::POST, "/api/v1/cron", &body).await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
    let id = res.value()["id"].as_str().unwrap().to_owned();
    let file = app.core.host.cron.content().unwrap();
    assert!(file.contains(&format!(
        "0 3 * * * root /usr/local/bin/yanto cron-run {id}\n"
    )));

    let o = c.get("/api/v1/cron").await.value();
    assert_eq!(o["file"]["state"], "in_sync");
    assert_eq!(
        o["entries"][0]["command"],
        "yanto backup yanto --keep-last 14"
    );
    assert_eq!(o["timezone"], "UTC");

    let res = c
        .json(
            Method::PATCH,
            &format!("/api/v1/cron/{id}"),
            &json!({"schedule": "@daily"}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(
        res.value()["command"],
        "yanto backup yanto --keep-last 14",
        "unchanged fields kept"
    );
    assert!(
        app.core
            .host
            .cron
            .content()
            .unwrap()
            .contains("@daily root")
    );

    let res = c
        .json(
            Method::POST,
            &format!("/api/v1/cron/{id}/toggle"),
            &json!({"enabled": false}),
        )
        .await;
    assert_eq!(res.value()["enabled"], false);
    assert!(
        app.core
            .host
            .cron
            .content()
            .unwrap()
            .contains(&format!("# disabled: {id}"))
    );

    // Field errors come back as 422 with the field name.
    let bad = c.json(Method::POST, "/api/v1/cron", &json!({"name": "x", "schedule": "61 * * * *", "run_as_user": "deploy", "command": "true"})).await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(bad.value()["error"]["fields"][0]["field"], "schedule");
    let dup = c.json(Method::POST, "/api/v1/cron", &body).await;
    assert_eq!(dup.status, StatusCode::CONFLICT);

    let p = c
        .json(
            Method::POST,
            "/api/v1/cron/preview",
            &json!({"schedule": "*/20 * * * *", "count": 3}),
        )
        .await
        .value();
    assert_eq!(p["next_runs"].as_array().unwrap().len(), 3);

    // Hand edit -> drift -> reapply.
    app.core
        .host
        .cron
        .set_managed(yanto_core::ports::ManagedFile::Present("evil\n".into()));
    assert_eq!(
        c.get("/api/v1/cron").await.value()["file"]["state"],
        "drift"
    );
    assert_eq!(
        c.json(Method::POST, "/api/v1/cron/reapply", &json!({}))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        c.get("/api/v1/cron").await.value()["file"]["state"],
        "in_sync"
    );

    let runs = c.get(&format!("/api/v1/cron/{id}/runs")).await;
    assert_eq!(
        (runs.status, runs.value().as_array().unwrap().len()),
        (StatusCode::OK, 0)
    );
    assert_eq!(
        c.json(Method::DELETE, &format!("/api/v1/cron/{id}"), &json!({}))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        c.get("/api/v1/cron").await.value()["entries"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn cron_dashboard_pages_and_forms() {
    let app = TestApp::new().await;
    let o = app.owner_client().await;
    let page = o.get("/cron").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.text();
    assert!(
        html.contains("New Cron Entry") && html.contains("Run as"),
        "{html}"
    );
    no_inline_script(&html);

    let form = [
        ("name", "backup <b>"),
        ("schedule", "*/5 * * * *"),
        ("run_as_user", "deploy"),
        ("command", "echo 100%"),
        ("overlap", "skip"),
        ("timeout_s", ""),
    ];
    let created = o.post_form("/cron", &form).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER);
    let page = o.get("/cron").await.text();
    assert!(page.contains("backup &#60;b&#62;"), "names are escaped");
    assert!(!page.contains("<b>"));
    no_inline_script(&page);
    let id = app
        .core
        .host
        .cron
        .content()
        .unwrap()
        .lines()
        .find_map(|l| l.split("cron-run ").nth(1).map(str::to_owned))
        .unwrap();

    // Validation errors re-render the form with the message and the typed values.
    let bad = o
        .post_form(
            "/cron",
            &[
                ("name", "x"),
                ("schedule", "61 * * * *"),
                ("run_as_user", "deploy"),
                ("command", "keep-me"),
            ],
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(bad.text().contains("minute: 61 is outside 0-59") && bad.text().contains("keep-me"));

    // htmx pieces.
    let prev = o
        .htmx(Method::POST, "/cron/preview", &[("schedule", "0 0 15 * 1")])
        .await;
    assert!(prev.text().contains("either matches") && prev.text().contains("UTC"));
    let prev = o
        .htmx(Method::POST, "/cron/preview", &[("schedule", "nope")])
        .await;
    assert!(prev.text().contains("class=\"error\""));
    let edit = o.get(&format!("/cron/{id}")).await;
    assert_eq!(edit.status, StatusCode::OK);
    assert!(edit.text().contains("Run history") && edit.text().contains("No runs recorded"));
    let put = o
        .htmx(
            Method::PUT,
            &format!("/cron/{id}"),
            &[
                ("name", "renamed"),
                ("schedule", "@hourly"),
                ("run_as_user", "deploy"),
                ("command", "true"),
            ],
        )
        .await;
    assert_eq!(put.header("hx-redirect"), Some("/cron"));
    assert!(
        app.core
            .host
            .cron
            .content()
            .unwrap()
            .contains("@hourly root")
    );
    let t = o
        .htmx(Method::POST, &format!("/cron/{id}/toggle"), &[])
        .await;
    assert_eq!(t.header("hx-redirect"), Some("/cron"));
    assert!(
        app.core
            .host
            .cron
            .content()
            .unwrap()
            .contains("# disabled:")
    );
    assert!(
        o.htmx(Method::GET, &format!("/cron/{id}/runs"), &[])
            .await
            .text()
            .contains("No runs")
    );
    app.core
        .host
        .cron
        .set_system_sources(vec![yanto_core::ports::CronSource {
            path: "/etc/cron.d/other".into(),
            kind: yanto_core::ports::CronSourceKind::CronD,
            content: yanto_core::ports::CronContent::Text("* * * * * root x\n".into()),
        }]);
    let files = o.htmx(Method::GET, "/cron/system-files", &[]).await.text();
    assert!(files.contains("/etc/cron.d/other") && files.contains("root x"));

    // Drift banner and re-apply.
    app.core
        .host
        .cron
        .set_managed(yanto_core::ports::ManagedFile::Present("edited\n".into()));
    assert!(o.get("/cron").await.text().contains("edited outside Yanto"));
    assert_eq!(
        o.post_form("/cron/reapply", &[]).await.status,
        StatusCode::SEE_OTHER
    );
    assert!(!o.get("/cron").await.text().contains("edited outside Yanto"));
    assert_eq!(
        o.htmx(Method::DELETE, &format!("/cron/{id}"), &[])
            .await
            .status,
        StatusCode::OK
    );

    // Members get a 403 page and no nav entry.
    let m = app.member_client("mia", &[]).await;
    assert_eq!(m.get("/cron").await.status, StatusCode::FORBIDDEN);
    assert!(!m.get("/").await.text().contains("href=\"/cron\""));
    assert!(o.get("/").await.text().contains("href=\"/cron\""));
}

#[tokio::test]
async fn authorized_keys_rest_and_dashboard_with_lockout_guard() {
    let app = TestApp::new().await;
    let c = admin(&app).await;
    let res = c
        .json(
            Method::POST,
            "/api/v1/authorized-keys",
            &json!({"name": "laptop", "public_key": KEY_A}),
        )
        .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
    let id = res.value()["key"]["id"].as_str().unwrap().to_owned();
    let file = String::from_utf8(app.core.host.authorized_keys.content().unwrap()).unwrap();
    assert!(
        file.contains("# BEGIN YANTO MANAGED KEYS") && file.contains(&format!("yanto:{id}:laptop"))
    );

    let bad = c
        .json(
            Method::POST,
            "/api/v1/authorized-keys",
            &json!({"public_key": format!("command=\"id\" {KEY_B}")}),
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let dup = c
        .json(
            Method::POST,
            "/api/v1/authorized-keys",
            &json!({"name": "again", "public_key": KEY_A}),
        )
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);

    let list = c.get("/api/v1/authorized-keys").await.value();
    assert_eq!(list["unix_user"], "deploy");
    assert_eq!(list["keys"].as_array().unwrap().len(), 1);
    assert_eq!(list["block_in_sync"], true);

    // Lockout guard: the only key cannot be removed without typing the user name.
    let refused = c
        .json(
            Method::DELETE,
            &format!("/api/v1/authorized-keys/{id}"),
            &json!({}),
        )
        .await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(refused.text().contains("Rule 1"));
    let forced = c
        .json(
            Method::DELETE,
            &format!("/api/v1/authorized-keys/{id}?confirm_user=deploy"),
            &json!({}),
        )
        .await;
    assert_eq!(forced.status, StatusCode::OK);
    assert_eq!(forced.value()["forced"], true);

    // Dashboard.
    let o = app.owner_client().await;
    let page = o.get("/ssh").await;
    assert_eq!(page.status, StatusCode::OK);
    no_inline_script(&page.text());
    let added = o
        .post_form("/ssh", &[("name", "ci"), ("public_key", KEY_B)])
        .await;
    assert_eq!(added.status, StatusCode::OK);
    assert!(added.text().contains("SHA256:"));
    let bad = o
        .post_form("/ssh", &[("name", ""), ("public_key", "ssh-dss AAAA")])
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(bad.text().contains("DSA"));
    let key_id = app
        .core
        .services
        .authorized_keys
        .list(&yanto_core::actor::Actor::cli())
        .await
        .unwrap()
        .keys[0]
        .id
        .clone();
    let refused = o.htmx(Method::DELETE, &format!("/ssh/{key_id}"), &[]).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(refused.text().contains("toast-msg"));
    let ok = o
        .htmx(
            Method::DELETE,
            &format!("/ssh/{key_id}?confirm_user=deploy"),
            &[],
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.header("hx-refresh"), Some("true"));
    assert_eq!(
        app.member_client("mia", &[]).await.get("/ssh").await.status,
        StatusCode::FORBIDDEN
    );
}

fn install_key(app: &TestApp) {
    let dir = app.core.dir.path().join("keys");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("app.key"), [5u8; 32]).unwrap();
}

#[tokio::test]
async fn yanto_backup_create_download_and_delete_over_rest() {
    let app = TestApp::new().await;
    install_key(&app);
    let owner = app.owner_user().await;
    let write = app
        .client()
        .with_bearer(&app.bearer(&owner, Scope::Write).await);
    let adm = admin(&app).await;

    let res = write
        .json(
            Method::POST,
            "/api/v1/backups",
            &json!({"kind": "yanto_sqlite"}),
        )
        .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.text());
    let b = res.value();
    assert_eq!(
        (
            b["kind"].as_str(),
            b["status"].as_str(),
            b["sensitive"].as_bool()
        ),
        (Some("yanto_sqlite"), Some("success"), Some(true))
    );
    let id = b["id"].as_str().unwrap().to_owned();
    let sha = b["sha256"].as_str().unwrap().to_owned();

    // Downloading needs `admin`, not `write`.
    assert_eq!(
        write
            .get(&format!("/api/v1/backups/{id}/download"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let dl = adm.get(&format!("/api/v1/backups/{id}/download")).await;
    assert_eq!(dl.status, StatusCode::OK);
    assert_eq!(
        dl.header("content-length"),
        Some(dl.body.len().to_string().as_str())
    );
    assert_eq!(dl.header("x-content-sha256"), Some(sha.as_str()));
    assert!(
        dl.header("content-disposition")
            .unwrap()
            .starts_with("attachment; filename=\"")
    );
    assert_eq!(dl.header("cache-control"), Some("no-store"));
    let out = app.core.dir.path().join("downloaded.tar.zst");
    std::fs::write(&out, &dl.body).unwrap();
    assert_eq!(
        backup::sha256_file(&out).unwrap(),
        sha,
        "the streamed bytes match the recorded hash"
    );
    let x = app.core.dir.path().join("x");
    backup::ensure_private_dir(&x).unwrap();
    assert!(backup::extract_yanto_archive(&out, &x).is_ok());

    let row = adm.get("/api/v1/backups").await.value();
    assert_eq!(row[0]["download_count"], 1);
    let audit = app
        .core
        .services
        .audit
        .list(&yanto_core::actor::Actor::cli(), &Default::default())
        .await
        .unwrap();
    let n = audit
        .entries
        .iter()
        .filter(|e| e.action == "backup.download")
        .count();
    assert_eq!(n, 1);
    let t = adm.get("/api/v1/backups/targets").await.value();
    assert_eq!(t[0]["kind"], "yanto_sqlite");

    // Project kinds are not available until M2 merges (TODO(M2-merge)).
    let p = adm
        .json(
            Method::POST,
            "/api/v1/backups",
            &json!({"kind": "project_volumes", "project_id": "prj_1"}),
        )
        .await;
    assert_eq!(p.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        adm.json(Method::POST, "/api/v1/backups", &json!({"kind": "r2"}))
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    assert_eq!(
        adm.json(Method::DELETE, &format!("/api/v1/backups/{id}"), &json!({}))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        adm.get(&format!("/api/v1/backups/{id}/download"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn members_never_reach_the_yanto_archive_and_see_only_their_project_backups() {
    let app = TestApp::new().await;
    install_key(&app);
    let owner = app.owner_user().await;
    let created = app
        .core
        .services
        .backups
        .create_yanto(
            &yanto_core::actor::Actor::cli(),
            yanto_core::backup::Trigger::Manual,
            None,
        )
        .await
        .unwrap();
    let carol = app
        .core
        .create_member("carol", &[("prj_1", &[Permission::Backups])])
        .await;
    let token = app
        .client()
        .with_bearer(&app.bearer(&carol, Scope::Admin).await);
    assert_eq!(
        token
            .get("/api/v1/backups")
            .await
            .value()
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        token
            .get(&format!("/api/v1/backups/{}/download", created.id))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        token
            .json(
                Method::POST,
                "/api/v1/backups",
                &json!({"kind": "yanto_sqlite"})
            )
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let _ = owner;
}

#[tokio::test]
async fn backups_dashboard_warns_streams_and_deletes() {
    let app = TestApp::new().await;
    install_key(&app);
    let o = app.owner_client().await;
    let page = o.get("/backups").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text().contains("keys/app.key") && page.text().contains("Sensitive"));
    no_inline_script(&page.text());
    let made = o.post_form("/backups", &[("kind", "yanto_sqlite")]).await;
    assert_eq!(made.status, StatusCode::SEE_OTHER, "{}", made.text());
    let page = o.get("/backups").await.text();
    assert!(
        page.contains("sensitive") && page.contains("-yanto.tar.zst") && page.contains("/download")
    );
    no_inline_script(&page);
    let id = app
        .core
        .services
        .backups
        .list(&yanto_core::actor::Actor::cli())
        .await
        .unwrap()[0]
        .id
        .clone();
    let dl = o.get(&format!("/backups/{id}/download")).await;
    assert_eq!(dl.status, StatusCode::OK);
    assert!(
        dl.header("content-disposition")
            .unwrap()
            .contains("attachment")
    );
    assert!(!dl.body.is_empty());
    let none = o
        .post_form("/backups", &[("kind", "project_volumes")])
        .await;
    assert_eq!(none.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        o.htmx(Method::DELETE, &format!("/backups/{id}"), &[])
            .await
            .status,
        StatusCode::OK
    );
    // A Member sees the page (empty) but no creation form.
    let m = app.member_client("mia", &[]).await;
    let mp = m.get("/backups").await;
    assert_eq!(mp.status, StatusCode::OK);
    assert!(!mp.text().contains("Create Yanto database backup"));
}
