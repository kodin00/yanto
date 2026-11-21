//! MCP over HTTP (`POST /mcp`): tool list snapshot, parity with the service layer, scope and
//! permission behavior of every tool, the `confirm` guard, and a JSON-RPC round trip.

use std::collections::BTreeMap;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use yanto_core::actor::{Access, Permission, Scope};
use yanto_core::service::all_operations;
use yanto_server::mcp::{ToolDef, ToolSet};
use yanto_server::testing::{TestApp, TestClient};

const ACCEPT: &str = "application/json, text/event-stream";

async fn rpc(
    c: &TestClient,
    bearer: Option<&str>,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let c = match bearer {
        Some(t) => c.clone().with_bearer(t),
        None => c.clone(),
    };
    let res = c
        .send(
            Method::POST,
            "/mcp",
            Some(("application/json", body.to_string())),
            &[("accept", ACCEPT)],
        )
        .await;
    let value = serde_json::from_slice(&res.body).unwrap_or(Value::Null);
    (res.status, value)
}

/// Call a tool; returns the `result` object (`isError`, `structuredContent`, ...).
async fn call(app: &TestApp, bearer: &str, tool: &str, args: Value) -> Value {
    let (status, v) = rpc(
        &app.client(),
        Some(bearer),
        "tools/call",
        json!({ "name": tool, "arguments": args }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tool}: {v}");
    v["result"].clone()
}

fn error_code(result: &Value) -> Option<&str> {
    if result["isError"] == json!(true) {
        result["structuredContent"]["error"]["code"].as_str()
    } else {
        None
    }
}

#[test]
fn tool_list_snapshot() {
    let set = ToolSet::standard();
    let mut out = String::from("name | operation | scope | class | confirm | open_world\n");
    for t in set.iter() {
        out.push_str(&format!(
            "{} | {} | {} | {:?} | {} | {}\n",
            t.name, t.op, t.scope, t.class, t.confirm, t.open_world
        ));
    }
    insta::assert_snapshot!(out);
}

#[test]
fn every_tool_maps_to_a_token_allowed_operation_with_the_same_scope() {
    let ops = all_operations();
    for t in ToolSet::standard().iter() {
        let op = ops
            .iter()
            .find(|o| o.name == t.op)
            .unwrap_or_else(|| panic!("{} calls unknown operation {}", t.name, t.op));
        assert!(
            op.token_allowed,
            "{} maps to a session-only operation",
            t.name
        );
        assert_eq!(
            op.min_scope, t.scope,
            "{}: scope differs from the service spec",
            t.name
        );
        assert!(t.name.starts_with("yanto_"), "{}", t.name);
    }
}

#[test]
fn exec_and_credential_management_are_not_exposed() {
    let ops = all_operations();
    for t in ToolSet::standard().iter() {
        for banned in [
            "exec",
            "terminal",
            "api_token",
            "session",
            "password_change",
            "download",
        ] {
            assert!(
                !t.name.contains(banned) && !t.op.contains(banned),
                "{} must not be exposed over MCP",
                t.name
            );
        }
        assert!(!t.op.starts_with("user_"), "{} manages users", t.name);
    }
    // Every session-only operation really has no tool.
    let tools: Vec<&str> = ToolSet::standard().iter().map(|t| t.op).collect();
    for op in ops.iter().filter(|o| !o.token_allowed) {
        assert!(!tools.contains(&op.name), "{} is session-only", op.name);
    }
}

#[tokio::test]
async fn http_round_trip_initialize_list_and_call() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let token = app.bearer(&owner, Scope::Admin).await;
    let c = app.client();

    let (st, init) = rpc(
        &c,
        Some(&token),
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "yanto");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    let (st, list) = rpc(&c, Some(&token), "tools/list", json!({})).await;
    assert_eq!(st, StatusCode::OK, "{list}");
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), ToolSet::standard().len());
    let del = tools
        .iter()
        .find(|t| t["name"] == "yanto_project_delete")
        .unwrap();
    assert_eq!(del["inputSchema"]["type"], "object");
    assert!(
        del["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("confirm"))
    );
    assert_eq!(del["annotations"]["destructiveHint"], true);
    let get = tools
        .iter()
        .find(|t| t["name"] == "yanto_project_get")
        .unwrap();
    assert_eq!(get["annotations"]["readOnlyHint"], true);

    // A read tool over HTTP, text block plus structuredContent.
    let r = call(&app, &token, "yanto_projects_list", json!({})).await;
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(r["structuredContent"]["items"], json!([]));
    assert!(r["content"][0]["text"].is_string());

    let r = call(&app, &token, "yanto_whoami", json!({})).await;
    assert_eq!(r["structuredContent"]["role"], "owner");
    assert_eq!(r["structuredContent"]["token"]["scope"], "admin");
}

#[tokio::test]
async fn write_tools_change_state_through_the_service_layer() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let token = app.bearer(&owner, Scope::Write).await;
    let r = call(
        &app,
        &token,
        "yanto_project_create",
        json!({ "name": "Web", "image_ref": "nginx:alpine" }),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let text = serde_json::to_string(&r).unwrap();
    assert!(
        !text.contains("ydp_"),
        "the Deploy Token must be redacted: {text}"
    );
    let id = r["structuredContent"]["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let r = call(&app, &token, "yanto_project_get", json!({ "id": id })).await;
    assert_eq!(r["structuredContent"]["name"], "Web");
    let r = call(&app, &token, "yanto_projects_list", json!({})).await;
    assert_eq!(r["structuredContent"]["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn bearer_is_required_and_cookies_are_ignored() {
    let app = TestApp::new().await;
    let owner = app.owner_client().await;
    let (st, _) = rpc(&app.client(), None, "tools/list", json!({})).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = rpc(&app.client(), Some("yat_nonsense"), "tools/list", json!({})).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // A signed-in dashboard session is not a credential here.
    let (st, _) = rpc(&owner, None, "tools/list", json!({})).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tools_above_the_token_scope_are_not_listed() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    for (scope, expect) in [
        (Scope::Read, Scope::Read),
        (Scope::Write, Scope::Write),
        (Scope::Admin, Scope::Admin),
    ] {
        let token = app.bearer(&owner, scope).await;
        let (_, list) = rpc(&app.client(), Some(&token), "tools/list", json!({})).await;
        let names: Vec<String> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        let want: Vec<String> = ToolSet::standard()
            .iter()
            .filter(|t| t.scope <= expect)
            .map(|t| t.name.to_owned())
            .collect();
        let mut got = names.clone();
        got.sort();
        let mut want = want;
        want.sort();
        assert_eq!(got, want, "scope {scope}");
    }
}

/// Minimal valid arguments for every tool, so each call reaches its service function.
fn sample_args() -> BTreeMap<&'static str, Value> {
    let p = json!({ "id": "prj_x" });
    let id = json!({ "id": "x" });
    let mut m = BTreeMap::new();
    for (k, v) in [
        ("yanto_whoami", json!({})),
        ("yanto_health", json!({})),
        ("yanto_system_usage", json!({})),
        ("yanto_audit_list", json!({})),
        ("yanto_cleanup_preview", json!({})),
        ("yanto_cleanup", json!({})),
        ("yanto_projects_list", json!({})),
        ("yanto_project_get", p.clone()),
        (
            "yanto_project_create",
            json!({"name":"N","image_ref":"nginx:alpine"}),
        ),
        ("yanto_project_update", json!({"id":"prj_x","name":"M"})),
        ("yanto_project_delete", p.clone()),
        ("yanto_project_compose_read", p.clone()),
        ("yanto_project_env_read", p.clone()),
        ("yanto_project_env_content_read", p.clone()),
        ("yanto_project_env_preview", json!({"content":"A=1"})),
        (
            "yanto_project_env_write",
            json!({"id":"prj_x","content":"A=1"}),
        ),
        (
            "yanto_project_env_variables_write",
            json!({"id":"prj_x","variables":[{"key":"A","value":"1"}]}),
        ),
        ("yanto_project_ports_list", p.clone()),
        ("yanto_project_ports_set", json!({"id":"prj_x","ports":[]})),
        ("yanto_project_deploy_token_reveal", p.clone()),
        ("yanto_project_deploy_token_rotate", p.clone()),
        ("yanto_project_stop", p.clone()),
        ("yanto_project_restart", p.clone()),
        ("yanto_project_deploy", p.clone()),
        ("yanto_project_rollback_preview", p.clone()),
        ("yanto_project_rollback", p.clone()),
        ("yanto_deployments_list", json!({})),
        ("yanto_deployment_get", id.clone()),
        ("yanto_deployment_logs", id.clone()),
        ("yanto_deployment_cancel", id.clone()),
        ("yanto_containers_list", json!({})),
        ("yanto_container_logs", id.clone()),
        ("yanto_container_start", id.clone()),
        ("yanto_container_stop", id.clone()),
        ("yanto_container_restart", id.clone()),
        ("yanto_backups_list", json!({})),
        ("yanto_backup_targets_list", json!({})),
        (
            "yanto_backup_create",
            json!({"kind":"project_volumes","project_id":"prj_x"}),
        ),
        ("yanto_backup_delete", id.clone()),
        ("yanto_deploy_key_list", json!({})),
        ("yanto_deploy_key_public_get", id.clone()),
        ("yanto_deploy_key_create", json!({"name":"k"})),
        ("yanto_deploy_key_delete", id.clone()),
        ("yanto_registry_credential_list", json!({})),
        (
            "yanto_registry_credential_create",
            json!({"name":"c","registry_host":"r.example.com","username":"u","secret":"s"}),
        ),
        (
            "yanto_registry_credential_update",
            json!({"id":"x","name":"d"}),
        ),
        ("yanto_registry_credential_delete", id.clone()),
        ("yanto_registry_status", json!({})),
        ("yanto_registry_enable", json!({})),
        ("yanto_registry_disable", json!({})),
        ("yanto_registry_users_list", json!({})),
        ("yanto_registry_user_create", json!({"username":"u"})),
        ("yanto_registry_user_delete", id.clone()),
        ("yanto_registry_repos_list", json!({})),
        ("yanto_registry_tags_list", json!({"repo":"r"})),
        ("yanto_registry_tag_delete", json!({"repo":"r","tag":"t"})),
        ("yanto_registry_gc", json!({})),
        ("yanto_cron_list", json!({})),
        ("yanto_cron_files_list", json!({})),
        ("yanto_cron_runs_list", id.clone()),
        (
            "yanto_cron_create",
            json!({"name":"n","schedule":"* * * * *","run_as_user":"root","command":"true"}),
        ),
        ("yanto_cron_update", json!({"id":"x","name":"m"})),
        ("yanto_cron_toggle", json!({"id":"x","enabled":true})),
        ("yanto_cron_delete", id.clone()),
        ("yanto_authorized_keys_list", json!({})),
        (
            "yanto_authorized_keys_add",
            json!({"public_key":"ssh-ed25519 AAAA"}),
        ),
        ("yanto_authorized_keys_delete", id.clone()),
        ("yanto_hostnames_list", json!({})),
        (
            "yanto_hostname_create",
            json!({"project_id":"prj_x","project_port_id":"ppt_x","hostname":"app.example.com","ingress_provider":"traefik"}),
        ),
        ("yanto_hostname_retry", id.clone()),
        ("yanto_hostname_enable", id.clone()),
        ("yanto_hostname_disable", id.clone()),
        ("yanto_hostname_delete", id.clone()),
        ("yanto_hostnames_diagnostics", json!({})),
        ("yanto_cloudflare_clients_list", json!({})),
        (
            "yanto_cloudflare_client_validate",
            json!({"account_id":"0123456789abcdef0123456789abcdef","api_token":"t"}),
        ),
        (
            "yanto_cloudflare_client_create",
            json!({"name":"main","account_id":"0123456789abcdef0123456789abcdef","api_token":"t"}),
        ),
        (
            "yanto_cloudflare_client_update",
            json!({"id":"x","name":"m"}),
        ),
        ("yanto_cloudflare_client_delete", id.clone()),
        ("yanto_cloudflare_zones_list", json!({"client_id":"x"})),
        ("yanto_cloudflare_tunnels_list", json!({})),
        (
            "yanto_cloudflare_tunnel_create",
            json!({"client_id":"x","name":"t1"}),
        ),
        ("yanto_cloudflare_tunnel_activate", id.clone()),
        ("yanto_cloudflare_tunnel_status", id.clone()),
        ("yanto_cloudflare_tunnel_delete", id.clone()),
        (
            "yanto_cloudflare_dns_list",
            json!({"client_id":"x","zone_id":"z"}),
        ),
        (
            "yanto_cloudflare_dns_create",
            json!({"client_id":"x","zone_id":"z","record_type":"A","name":"a.example.com","content":"192.0.2.1"}),
        ),
        (
            "yanto_cloudflare_dns_update",
            json!({"client_id":"x","zone_id":"z","record_id":"r","record_type":"A","name":"a.example.com","content":"192.0.2.1"}),
        ),
        (
            "yanto_cloudflare_dns_delete",
            json!({"client_id":"x","zone_id":"z","record_id":"r"}),
        ),
        ("yanto_frps_overview", json!({})),
        ("yanto_frps_settings_update", json!({"bind_port":7000})),
        ("yanto_daemons_status", json!({})),
        ("yanto_traefik_status", json!({})),
        ("yanto_system_logs", json!({})),
        ("yanto_project_databases_list", p.clone()),
        (
            "yanto_project_databases_set",
            json!({"id":"prj_x","databases":[]}),
        ),
        ("yanto_backup_restore", id.clone()),
    ] {
        m.insert(k, v);
    }
    m
}

#[test]
fn sample_arguments_cover_every_tool() {
    let samples = sample_args();
    let set = ToolSet::standard();
    for t in set.iter() {
        assert!(
            samples.contains_key(t.name),
            "add sample arguments for {}",
            t.name
        );
    }
    assert_eq!(samples.len(), set.len(), "sample table has a stale tool");
}

fn expected_denial(def: &ToolDef, scope: Option<Scope>, member: bool) -> Option<&'static str> {
    let ops = all_operations();
    let op = ops.iter().find(|o| o.name == def.op).unwrap();
    if scope.is_some_and(|s| s < op.min_scope) {
        return Some("forbidden");
    }
    match op.access {
        Access::Owner if member => Some("forbidden"),
        Access::Project(_) if member => Some("not_found"),
        _ => None,
    }
}

#[tokio::test]
async fn every_tool_enforces_scope_and_ownership_exactly_like_the_service_spec() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    // A Member with no grants at all.
    let member = app.core.create_member("mallory", &[]).await;
    let samples = sample_args();

    let mut profiles: Vec<(String, String, Option<Scope>, bool)> = Vec::new();
    for s in Scope::ALL {
        profiles.push((
            format!("owner-{s}"),
            app.bearer(&owner, s).await,
            Some(s),
            false,
        ));
        profiles.push((
            format!("member-{s}"),
            app.bearer(&member, s).await,
            Some(s),
            true,
        ));
    }
    for t in ToolSet::standard().iter() {
        let mut args = samples[t.name].clone();
        args["confirm"] = json!(true);
        for (label, bearer, scope, is_member) in &profiles {
            let r = call(&app, bearer, t.name, args.clone()).await;
            let got = error_code(&r);
            let want = expected_denial(t, *scope, *is_member);
            match want {
                Some(code) => assert_eq!(got, Some(code), "{label} -> {} ({r})", t.name),
                None => assert!(
                    !matches!(
                        got,
                        Some("forbidden" | "unauthenticated" | "confirmation_required")
                    ),
                    "{label} -> {} unexpectedly denied: {r}",
                    t.name
                ),
            }
        }
    }
}

#[tokio::test]
async fn read_tokens_cannot_write_and_unconfirmed_calls_are_refused() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let read = app.bearer(&owner, Scope::Read).await;
    let write = app.bearer(&owner, Scope::Write).await;
    let admin = app.bearer(&owner, Scope::Admin).await;

    // read < write
    let r = call(
        &app,
        &read,
        "yanto_project_create",
        json!({"name":"A","image_ref":"nginx"}),
    )
    .await;
    assert_eq!(error_code(&r), Some("forbidden"));
    // write < admin, even with confirm
    let r = call(
        &app,
        &write,
        "yanto_project_delete",
        json!({"id":"prj_x","confirm":true}),
    )
    .await;
    assert_eq!(error_code(&r), Some("forbidden"));
    // admin without confirm is refused before the service runs
    let r = call(&app, &admin, "yanto_project_delete", json!({"id":"prj_x"})).await;
    assert_eq!(error_code(&r), Some("confirmation_required"));
    let r = call(
        &app,
        &admin,
        "yanto_project_delete",
        json!({"id":"prj_x","confirm":false}),
    )
    .await;
    assert_eq!(error_code(&r), Some("confirmation_required"));
    // a read token gets forbidden, not a hint that a confirm flag exists
    let r = call(&app, &read, "yanto_project_delete", json!({"id":"prj_x"})).await;
    assert_eq!(error_code(&r), Some("forbidden"));
    // with confirm the call reaches the service (unknown Project => not_found)
    let r = call(
        &app,
        &admin,
        "yanto_project_delete",
        json!({"id":"prj_x","confirm":true}),
    )
    .await;
    assert_eq!(error_code(&r), Some("not_found"));
}

#[tokio::test]
async fn members_only_reach_projects_and_permissions_they_were_granted() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let admin = app.bearer(&owner, Scope::Admin).await;
    let mk = |n: &str| json!({"name": n, "image_ref": "nginx:alpine"});
    let a = call(&app, &admin, "yanto_project_create", mk("Alpha")).await;
    let b = call(&app, &admin, "yanto_project_create", mk("Bravo")).await;
    let a = a["structuredContent"]["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let b = b["structuredContent"]["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let ga = [(a.as_str(), &[Permission::Deploy][..])];
    let alice = app.core.create_member("alice", &ga).await;
    let token = app.bearer(&alice, Scope::Admin).await;

    // Sees only her Project.
    let r = call(&app, &token, "yanto_projects_list", json!({})).await;
    let items = r["structuredContent"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], json!(a));
    // Ungranted Project: indistinguishable from missing.
    for tool in ["yanto_project_get", "yanto_deployment_get"] {
        let r = call(&app, &token, tool, json!({"id": b})).await;
        assert_eq!(error_code(&r), Some("not_found"), "{tool}");
    }
    let r = call(&app, &token, "yanto_project_deploy", json!({"id": b})).await;
    assert_eq!(error_code(&r), Some("not_found"));
    // Granted Project but missing permission: forbidden.
    let r = call(&app, &token, "yanto_project_env_read", json!({"id": a})).await;
    assert_eq!(error_code(&r), Some("forbidden"));
    let r = call(
        &app,
        &token,
        "yanto_project_stop",
        json!({"id": a, "confirm": true}),
    )
    .await;
    assert_eq!(error_code(&r), Some("forbidden"));
    // Granted permission works.
    let r = call(&app, &token, "yanto_project_deploy", json!({"id": a})).await;
    assert_eq!(error_code(&r), None, "{r}");
    // Owner-only tools stay closed to Members whatever the scope.
    let r = call(&app, &token, "yanto_project_create", mk("Mine")).await;
    assert_eq!(error_code(&r), Some("forbidden"));
}

#[tokio::test]
async fn sensitive_values_stay_redacted_over_mcp() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let admin = app.bearer(&owner, Scope::Admin).await;
    let r = call(
        &app,
        &admin,
        "yanto_project_create",
        json!({"name":"Env","image_ref":"nginx"}),
    )
    .await;
    let id = r["structuredContent"]["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &app,
        &admin,
        "yanto_project_env_write",
        json!({"id": id, "content": "SECRET_KEY=hunter2hunter2\n"}),
    )
    .await;
    for tool in ["yanto_project_env_read", "yanto_project_env_content_read"] {
        let r = call(&app, &admin, tool, json!({"id": id})).await;
        let text = serde_json::to_string(&r).unwrap();
        assert!(
            !text.contains("hunter2hunter2"),
            "{tool} leaked a value: {text}"
        );
        assert!(
            text.contains("untrusted"),
            "{tool} must label untrusted output"
        );
    }
    // Revealing the Deploy Token needs confirm and admin.
    let r = call(
        &app,
        &admin,
        "yanto_project_deploy_token_reveal",
        json!({"id": id}),
    )
    .await;
    assert_eq!(error_code(&r), Some("confirmation_required"));
    let r = call(
        &app,
        &admin,
        "yanto_project_deploy_token_reveal",
        json!({"id": id, "confirm": true}),
    )
    .await;
    assert!(
        r["structuredContent"]["deploy_token"]
            .as_str()
            .unwrap()
            .starts_with("ydp_")
    );
}

#[tokio::test]
async fn invalid_arguments_and_unknown_tools_are_structured_errors() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let token = app.bearer(&owner, Scope::Admin).await;
    let r = call(&app, &token, "yanto_project_get", json!({})).await;
    assert_eq!(error_code(&r), Some("validation_failed"));
    assert_eq!(
        r["structuredContent"]["error"]["fields"][0]["field"],
        "arguments"
    );
    let (_, v) = rpc(
        &app.client(),
        Some(&token),
        "tools/call",
        json!({"name": "yanto_container_exec", "arguments": {}}),
    )
    .await;
    assert!(
        v["error"].is_object(),
        "unknown tool is a JSON-RPC error: {v}"
    );
}

#[tokio::test]
async fn mutations_are_audited_on_the_mcp_channel() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let token = app.bearer(&owner, Scope::Write).await;
    call(
        &app,
        &token,
        "yanto_project_create",
        json!({"name":"Aud","image_ref":"nginx"}),
    )
    .await;
    let page = app
        .core
        .services
        .audit
        .list(&yanto_core::actor::Actor::cli(), &Default::default())
        .await
        .unwrap();
    let e = page
        .entries
        .iter()
        .find(|e| e.action == "project.create")
        .expect("audited");
    assert_eq!(e.channel, "mcp");
    assert_eq!(e.actor_type, "api_token");
}

#[tokio::test]
async fn ingress_and_host_tools_follow_scope_confirm_and_ownership() {
    let app = TestApp::new().await;
    let owner = app.owner_user().await;
    let read = app.bearer(&owner, Scope::Read).await;
    let write = app.bearer(&owner, Scope::Write).await;
    let admin = app.bearer(&owner, Scope::Admin).await;

    // Read tools are open to a read token; every mutating one is not.
    for tool in [
        "yanto_hostnames_list",
        "yanto_cloudflare_clients_list",
        "yanto_frps_overview",
        "yanto_daemons_status",
        "yanto_traefik_status",
        "yanto_system_logs",
    ] {
        let r = call(&app, &read, tool, json!({})).await;
        assert_eq!(error_code(&r), None, "{tool}: {r}");
    }
    for (tool, args) in [
        ("yanto_hostname_retry", json!({"id": "x"})),
        (
            "yanto_cloudflare_dns_create",
            json!({"client_id":"x","zone_id":"z","record_type":"A","name":"a.example.com","content":"192.0.2.1"}),
        ),
        ("yanto_frps_settings_update", json!({"bind_port": 7000})),
        (
            "yanto_project_databases_set",
            json!({"id":"prj_x","databases":[]}),
        ),
    ] {
        let r = call(&app, &read, tool, args).await;
        assert_eq!(error_code(&r), Some("forbidden"), "{tool}");
    }

    // Admin-scope tools: a write token is refused even with confirm, an admin token needs confirm.
    for (tool, args) in [
        ("yanto_hostname_delete", json!({"id": "x"})),
        ("yanto_cloudflare_client_delete", json!({"id": "x"})),
        ("yanto_cloudflare_tunnel_delete", json!({"id": "x"})),
        (
            "yanto_cloudflare_dns_delete",
            json!({"client_id":"x","zone_id":"z","record_id":"r"}),
        ),
        ("yanto_backup_restore", json!({"id": "x"})),
    ] {
        let mut confirmed = args.clone();
        confirmed["confirm"] = json!(true);
        let r = call(&app, &write, tool, confirmed.clone()).await;
        assert_eq!(
            error_code(&r),
            Some("forbidden"),
            "{tool} with a write token"
        );
        let r = call(&app, &admin, tool, args).await;
        assert_eq!(error_code(&r), Some("confirmation_required"), "{tool}");
        let r = call(&app, &admin, tool, confirmed).await;
        assert_eq!(
            error_code(&r),
            Some("not_found"),
            "{tool} reaches the service"
        );
    }

    // frps settings restart the daemon to take effect: write scope, but confirm is mandatory.
    let r = call(
        &app,
        &write,
        "yanto_frps_settings_update",
        json!({"bind_port": 7000}),
    )
    .await;
    assert_eq!(error_code(&r), Some("confirmation_required"));

    // Host Daemon control is not offered at all.
    assert!(
        ToolSet::standard()
            .iter()
            .all(|t| t.name != "yanto_daemon_control" && t.op != "daemon_control")
    );

    // A Member with a Hostnames grant reaches only the Hostname tools of that Project.
    let r = call(
        &app,
        &admin,
        "yanto_project_create",
        json!({"name":"Alpha","image_ref":"nginx:alpine"}),
    )
    .await;
    let a = r["structuredContent"]["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = call(
        &app,
        &admin,
        "yanto_project_create",
        json!({"name":"Bravo","image_ref":"nginx:alpine"}),
    )
    .await;
    let b = r["structuredContent"]["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let grants = [(a.as_str(), &[Permission::Hostnames][..])];
    let alice = app.core.create_member("alice", &grants).await;
    let token = app.bearer(&alice, Scope::Admin).await;
    let r = call(
        &app,
        &token,
        "yanto_hostnames_list",
        json!({"project_id": a}),
    )
    .await;
    assert_eq!(error_code(&r), None, "{r}");
    let r = call(
        &app,
        &token,
        "yanto_hostnames_list",
        json!({"project_id": b}),
    )
    .await;
    assert_eq!(error_code(&r), Some("not_found"));
    let create = |p: &str| json!({"project_id": p, "project_port_id": "ppt_x", "hostname": "app.example.com", "ingress_provider": "traefik"});
    let r = call(&app, &token, "yanto_hostname_create", create(&b)).await;
    assert_eq!(error_code(&r), Some("not_found"));
    for (tool, args) in [
        ("yanto_cloudflare_clients_list", json!({})),
        ("yanto_frps_overview", json!({})),
        ("yanto_daemons_status", json!({})),
        ("yanto_traefik_status", json!({})),
        ("yanto_system_logs", json!({})),
        (
            "yanto_project_databases_set",
            json!({"id": a, "databases": [], "confirm": true}),
        ),
        ("yanto_backup_restore", json!({"id": "x", "confirm": true})),
    ] {
        let r = call(&app, &token, tool, args).await;
        assert_eq!(error_code(&r), Some("forbidden"), "{tool} for a Member");
    }
}
