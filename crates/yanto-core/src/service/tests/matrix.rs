//! Permission matrix: every actor profile against every M1 operation.
//!
//! The grid below is the specification (docs/rewrite/03 a.1, a.2, a.5) written out by
//! hand, one row per operation. It is checked two ways:
//!
//! 1. `authorize()` against the operation's `OpSpec`;
//! 2. the real service function, so a missing `authorize` call inside a service fails here.
//!
//! The test also fails when `all_operations()` contains an operation without a grid row
//! (a new operation cannot ship without declaring who may call it) and vice versa.

use std::collections::BTreeSet;

use super::backups::{project_backup_fixture, yanto_row_fixture};
use crate::ErrorKind;
use crate::actor::{Actor, Permission, Scope};
use crate::authkeys::test_keys;
use crate::backup::{BackupKind, Trigger};
use crate::cron::{EntryInput, Overlap};
use crate::db::repo::users::UserStatus;
use crate::error::CoreError;
use crate::service::all_operations;
use crate::service::authorized_keys::AddKey;
use crate::service::backups::Retention;
use crate::service::system::CleanupSelection;
use crate::service::users::GrantInput;
use crate::testing::{TestCore, TestUser};

/// Actor profiles, in grid column order.
const PROFILES: [&str; 9] = [
    "owner", "member", "otok-r", "otok-w", "otok-a", "mtok-r", "mtok-w", "mtok-a", "cli",
];

/// `Y` = allowed, `N` = denied (403). Columns follow [`PROFILES`].
const GRID: &[(&str, &str)] = &[
    //                          own mem  oR  oW  oA  mR  mW  mA  cli
    // Public operations validate their own credential; no actor is needed.
    ("owner_setup", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("session_login", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("account_preview", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("account_setup", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    // Session-only (dashboard/CLI): never reachable with an API Token.
    ("session_logout", "Y   Y    N   N   N   N   N   N   Y"),
    ("session_list", "Y   Y    N   N   N   N   N   N   Y"),
    ("session_revoke", "Y   Y    N   N   N   N   N   N   Y"),
    ("password_change", "Y   Y    N   N   N   N   N   N   Y"),
    ("api_token_create", "Y   Y    N   N   N   N   N   N   Y"),
    ("api_token_list", "Y   Y    N   N   N   N   N   N   Y"),
    ("api_token_revoke", "Y   Y    N   N   N   N   N   N   Y"),
    // Owner-only, session or CLI.
    ("setup_code_issue", "Y   N    N   N   N   N   N   N   Y"),
    ("owner_reset", "Y   N    N   N   N   N   N   N   Y"),
    // Owner-only, token needs admin scope (and the token's user must be the Owner).
    ("user_list", "Y   N    N   N   Y   N   N   N   Y"),
    ("user_get", "Y   N    N   N   Y   N   N   N   Y"),
    ("user_create", "Y   N    N   N   Y   N   N   N   Y"),
    ("user_access_update", "Y   N    N   N   Y   N   N   N   Y"),
    ("user_status_update", "Y   N    N   N   Y   N   N   N   Y"),
    ("user_reset_link", "Y   N    N   N   Y   N   N   N   Y"),
    ("user_delete", "Y   N    N   N   Y   N   N   N   Y"),
    // Any authenticated actor, read scope.
    ("whoami", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("audit_list", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    // M3 containers. The spec only checks scope and session-only rules; the per-container rule
    // (Project grant, or Owner for containers outside any Project) lives in the service and is
    // covered by `service/tests/containers.rs`. A Member passing here gets 404, never 403.
    ("containers_list", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("container_get", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("container_logs", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    (
        "container_logs_stream",
        "Y   Y    Y   Y   Y   Y   Y   Y   Y",
    ),
    ("container_start", "Y   Y    N   Y   Y   N   Y   Y   Y"),
    ("container_stop", "Y   Y    N   N   Y   N   N   Y   Y"),
    ("container_restart", "Y   Y    N   N   Y   N   N   Y   Y"),
    // Terminal: dashboard-only (D24), so no token ever passes.
    (
        "container_exec_session",
        "Y   Y    N   N   N   N   N   N   Y",
    ),
    // M3 system. Owner-only; read-scope tokens of the Owner may read, only admin may prune.
    ("system_usage", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("system_logs", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("system_logs_stream", "Y   N    Y   Y   Y   N   N   N   Y"),
    (
        "system_cleanup_preview",
        "Y   N    Y   Y   Y   N   N   N   Y",
    ),
    ("system_cleanup", "Y   N    N   N   Y   N   N   N   Y"),
    ("health", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    // Host management (M5). All Owner-only; tokens need the scope in the OpSpec.
    ("cron_list", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("cron_files_list", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("cron_runs_list", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("cron_preview", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("cron_create", "Y   N    N   N   Y   N   N   N   Y"),
    ("cron_update", "Y   N    N   N   Y   N   N   N   Y"),
    ("cron_toggle", "Y   N    N   Y   Y   N   N   N   Y"),
    ("cron_delete", "Y   N    N   N   Y   N   N   N   Y"),
    ("cron_reapply", "Y   N    N   N   Y   N   N   N   Y"),
    ("cron_run", "Y   N    N   N   N   N   N   N   Y"),
    ("authorized_keys_list", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("authorized_keys_add", "Y   N    N   N   Y   N   N   N   Y"),
    (
        "authorized_keys_delete",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    // Backups: the member (alice) has access to prj_1 without `backups`, so Project ops are denied.
    ("backups_list", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("backup_targets_list", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("backup_create", "Y   N    N   Y   Y   N   N   N   Y"),
    ("backup_create_yanto", "Y   N    N   Y   Y   N   N   N   Y"),
    ("backup_download", "Y   N    N   N   Y   N   N   N   Y"),
    (
        "backup_download_yanto",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    ("backup_delete", "Y   N    N   N   Y   N   N   N   Y"),
    ("backup_delete_yanto", "Y   N    N   N   Y   N   N   N   Y"),
    ("backup_restore", "Y   N    N   N   Y   N   N   N   Y"),
    ("backup_prune", "Y   N    N   N   N   N   N   N   Y"),
    // M2. Member = alice with only `deploy` on prj_1. Project-scoped rows use prj_1 for the
    // spec check; the per-permission cells (404 without access, 403 with the wrong permission)
    // are in `matrix_projects.rs`.
    ("project_list", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("project_get", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("project_create", "Y   N    N   Y   Y   N   N   N   Y"),
    ("project_update", "Y   N    N   Y   Y   N   N   N   Y"),
    ("project_delete", "Y   N    N   N   Y   N   N   N   Y"),
    ("project_compose_read", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("project_env_read", "Y   N    Y   Y   Y   N   N   N   Y"),
    (
        "project_env_content_read",
        "Y   N    Y   Y   Y   N   N   N   Y",
    ),
    ("project_env_preview", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("project_env_write", "Y   N    N   Y   Y   N   N   N   Y"),
    (
        "project_env_variables_write",
        "Y   N    N   Y   Y   N   N   N   Y",
    ),
    ("project_ports_list", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("project_ports_set", "Y   N    N   Y   Y   N   N   N   Y"),
    (
        "project_databases_list",
        "Y   Y    Y   Y   Y   Y   Y   Y   Y",
    ),
    (
        "project_databases_set",
        "Y   N    N   Y   Y   N   N   N   Y",
    ),
    (
        "project_deploy_token_reveal",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    (
        "project_deploy_token_rotate",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    ("project_stop", "Y   N    N   N   Y   N   N   N   Y"),
    ("project_restart", "Y   N    N   N   Y   N   N   N   Y"),
    ("project_deploy", "Y   Y    N   Y   Y   N   Y   Y   Y"),
    (
        "project_rollback_preview",
        "Y   Y    Y   Y   Y   Y   Y   Y   Y",
    ),
    ("project_rollback", "Y   Y    N   N   Y   N   N   Y   Y"),
    (
        "project_reset_checkout",
        "Y   Y    N   N   Y   N   N   Y   Y",
    ),
    ("deployments_list", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("deployment_get", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("deployment_logs", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    (
        "deployment_logs_stream",
        "Y   Y    Y   Y   Y   Y   Y   Y   Y",
    ),
    ("deployment_cancel", "Y   Y    N   Y   Y   N   Y   Y   Y"),
    (
        "deployments_events_stream",
        "Y   Y    Y   Y   Y   Y   Y   Y   Y",
    ),
    ("deploy_webhook", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("github_webhook", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("deploy_key_list", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    (
        "deploy_key_public_get",
        "Y   Y    Y   Y   Y   Y   Y   Y   Y",
    ),
    ("deploy_key_create", "Y   N    N   N   Y   N   N   N   Y"),
    ("deploy_key_import", "Y   N    N   N   Y   N   N   N   Y"),
    ("deploy_key_delete", "Y   N    N   N   Y   N   N   N   Y"),
    (
        "registry_credential_list",
        "Y   Y    Y   Y   Y   Y   Y   Y   Y",
    ),
    (
        "registry_credential_create",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    (
        "registry_credential_update",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    (
        "registry_credential_delete",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    // M6 Hosted Registry: Owner-only; scope per OpSpec (read < write < admin).
    ("registry_status", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("registry_users_list", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("registry_repos_list", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("registry_tags_list", "Y   N    Y   Y   Y   N   N   N   Y"),
    (
        "registry_tag_delete_preview",
        "Y   N    Y   Y   Y   N   N   N   Y",
    ),
    ("registry_gc_status", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("registry_user_create", "Y   N    N   Y   Y   N   N   N   Y"),
    (
        "registry_user_password_set",
        "Y   N    N   Y   Y   N   N   N   Y",
    ),
    ("registry_user_delete", "Y   N    N   N   Y   N   N   N   Y"),
    ("registry_enable", "Y   N    N   N   Y   N   N   N   Y"),
    ("registry_disable", "Y   N    N   N   Y   N   N   N   Y"),
    (
        "registry_settings_update",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    ("registry_restart", "Y   N    N   N   Y   N   N   N   Y"),
    ("registry_rerender", "Y   N    N   N   Y   N   N   N   Y"),
    ("registry_tag_delete", "Y   N    N   N   Y   N   N   N   Y"),
    ("registry_gc_preview", "Y   N    N   N   Y   N   N   N   Y"),
    ("registry_gc", "Y   N    N   N   Y   N   N   N   Y"),
    // M7 ingress and FRP. Owner-only except the Hostname operations, which are Project-scoped
    // (`hostnames` permission; alice has only `deploy`). Reveal operations are session-only.
    ("daemons_status", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("daemon_control", "Y   N    N   N   Y   N   N   N   Y"),
    (
        "cloudflare_clients_list",
        "Y   N    Y   Y   Y   N   N   N   Y",
    ),
    (
        "cloudflare_client_validate",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    (
        "cloudflare_client_create",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    (
        "cloudflare_client_update",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    (
        "cloudflare_client_delete",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    (
        "cloudflare_zones_list",
        "Y   N    Y   Y   Y   N   N   N   Y",
    ),
    (
        "cloudflare_tunnels_list",
        "Y   N    Y   Y   Y   N   N   N   Y",
    ),
    (
        "cloudflare_tunnel_create",
        "Y   N    N   Y   Y   N   N   N   Y",
    ),
    (
        "cloudflare_tunnel_activate",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    (
        "cloudflare_tunnel_status",
        "Y   N    Y   Y   Y   N   N   N   Y",
    ),
    (
        "cloudflare_tunnel_delete",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    (
        "cloudflare_tunnel_token_reveal",
        "Y   N    N   N   N   N   N   N   Y",
    ),
    ("cloudflare_dns_list", "Y   N    Y   Y   Y   N   N   N   Y"),
    (
        "cloudflare_dns_create",
        "Y   N    N   Y   Y   N   N   N   Y",
    ),
    (
        "cloudflare_dns_update",
        "Y   N    N   Y   Y   N   N   N   Y",
    ),
    (
        "cloudflare_dns_delete",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    ("frps_overview", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("frps_settings_update", "Y   N    N   Y   Y   N   N   N   Y"),
    ("frps_config_restore", "Y   N    N   Y   Y   N   N   N   Y"),
    ("frps_token_reveal", "Y   N    N   N   N   N   N   N   Y"),
    ("traefik_status", "Y   N    Y   Y   Y   N   N   N   Y"),
    ("hostnames_resync", "Y   N    N   Y   Y   N   N   N   Y"),
    (
        "hostnames_public_ip_set",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    (
        "hostname_registry_manage",
        "Y   N    N   N   Y   N   N   N   Y",
    ),
    ("hostnames_list", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    ("hostnames_options", "Y   Y    Y   Y   Y   Y   Y   Y   Y"),
    (
        "hostnames_diagnostics",
        "Y   Y    Y   Y   Y   Y   Y   Y   Y",
    ),
    ("hostname_create", "Y   N    N   Y   Y   N   N   N   Y"),
    ("hostname_retry", "Y   N    N   Y   Y   N   N   N   Y"),
    ("hostname_enable", "Y   N    N   Y   Y   N   N   N   Y"),
    ("hostname_disable", "Y   N    N   Y   Y   N   N   N   Y"),
    ("hostname_delete", "Y   N    N   N   Y   N   N   N   Y"),
];

/// Project-scoped operations are checked against prj_1 (Alice's only Project).
fn project_for(op: &crate::actor::OpSpec) -> Option<&'static str> {
    matches!(op.access, crate::actor::Access::Project(_)).then_some("prj_1")
}

fn grid_row(name: &str) -> Vec<bool> {
    let (_, row) = GRID
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("operation {name} has no permission-matrix row"));
    let cells: Vec<bool> = row.split_whitespace().map(|c| c == "Y").collect();
    assert_eq!(cells.len(), PROFILES.len(), "row {name}");
    cells
}

#[test]
fn grid_and_operation_registry_match_exactly() {
    let ops: BTreeSet<&str> = all_operations().iter().map(|o| o.name).collect();
    let grid: BTreeSet<&str> = GRID.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        ops, grid,
        "operations without a row, or rows without an operation"
    );
    assert_eq!(
        all_operations().len(),
        ops.len(),
        "duplicate operation names"
    );
}

struct Fixture {
    core: TestCore,
    owner: TestUser,
    alice: TestUser,
    bob: TestUser,
    cid: String,
    /// Existing Backup rows (Project kind on prj_1, and the Yanto DB) for the id-based operations.
    project_backup: String,
    yanto_backup: String,
    /// A queued Deployment of prj_1.
    dep: String,
    key: String,
    cred: String,
}

fn cron_input() -> EntryInput {
    EntryInput {
        name: "matrix".into(),
        schedule: "@daily".into(),
        run_as_user: "deploy".into(),
        command: "true".into(),
        overlap: Overlap::Allow,
        timeout_s: None,
        enabled: true,
    }
}

async fn fixture() -> Fixture {
    let core = TestCore::new().await;
    core.ensure_project("prj_9").await;
    let owner = core.setup_owner().await;
    let alice = core
        .create_member("alice", &[("prj_1", &[Permission::Deploy])])
        .await;
    let bob = core.create_member("bob", &[]).await;
    let cid = core.docker.add_container("web", "running", &[]);
    let project_backup = project_backup_fixture(&core, "prj_1").await;
    let yanto_backup = yanto_row_fixture(&core).await;
    let cli = Actor::cli();
    let dep = core
        .services
        .deployments
        .deploy(&cli, "prj_1", &Default::default())
        .await
        .expect("seed deployment")
        .deployment
        .id;
    let key = core
        .services
        .deploy_keys
        .create(&cli, "seed-key")
        .await
        .expect("seed key")
        .id;
    let cred = core
        .services
        .registry_credentials
        .create(
            &cli,
            "seed-cred",
            "ghcr.io",
            "bot",
            &crate::secrets::Secret::new("s3cret-pass"),
        )
        .await
        .expect("seed credential")
        .id;
    Fixture {
        core,
        owner,
        alice,
        bob,
        cid,
        project_backup,
        yanto_backup,
        dep,
        key,
        cred,
    }
}

async fn actor_for(f: &Fixture, profile: &str) -> Actor {
    let scope = |c: char| match c {
        'r' => Scope::Read,
        'w' => Scope::Write,
        _ => Scope::Admin,
    };
    match profile {
        "owner" => f.core.session_actor(&f.owner).await,
        "member" => f.core.session_actor(&f.alice).await,
        "cli" => Actor::cli(),
        p if p.starts_with("otok-") => {
            f.core
                .token_actor(&f.owner, scope(p.chars().last().unwrap()))
                .await
                .1
        }
        p if p.starts_with("mtok-") => {
            f.core
                .token_actor(&f.alice, scope(p.chars().last().unwrap()))
                .await
                .1
        }
        _ => unreachable!(),
    }
}

/// Call the real service function for `op` with arguments that would succeed for an
/// authorized caller. The result only needs to distinguish "denied" from "got past authorization".
async fn invoke(f: &Fixture, op: &str, a: &Actor) -> Result<(), CoreError> {
    let s = &f.core.services;
    let origin = f.core.origin();
    match op {
        "owner_setup" => s
            .auth
            .owner_setup(
                &origin,
                "AAAA-AAAA-AAAA-AAAA",
                "x1",
                "long-enough-password",
                None,
            )
            .await
            .map(|_| ()),
        "session_login" => s
            .auth
            .login(&origin, "alice", "wrong-password-xx", None)
            .await
            .map(|_| ()),
        "account_preview" => s.auth.account_preview(&origin, "nope").await.map(|_| ()),
        "account_setup" => s
            .auth
            .account_setup(&origin, "nope", "long-enough-password", None)
            .await
            .map(|_| ()),
        "session_logout" => s.auth.logout(a, "missing").await,
        "session_list" => s.auth.session_list(a, "missing").await.map(|_| ()),
        "session_revoke" => s.auth.session_revoke(a, "missing").await,
        "password_change" => {
            s.auth
                .password_change(a, "missing", "wrong-current-pass", "another-long-password")
                .await
        }
        "api_token_create" => s
            .tokens
            .create(a, "matrix", Scope::Read, None)
            .await
            .map(|_| ()),
        "api_token_list" => s.tokens.list(a).await.map(|_| ()),
        "api_token_revoke" => s.tokens.revoke(a, "tok_missing").await,
        "setup_code_issue" => s.auth.issue_setup_code(a).await.map(|_| ()),
        "owner_reset" => s.auth.owner_reset(a).await.map(|_| ()),
        "user_list" => s.users.list(a).await.map(|_| ()),
        "user_get" => s.users.get(a, &f.bob.id).await.map(|_| ()),
        "user_create" => s.users.create(a, "newbie", &[]).await.map(|_| ()),
        "user_access_update" => s
            .users
            .update_access(
                a,
                &f.bob.id,
                &[GrantInput {
                    project_id: "prj_9".into(),
                    permissions: vec![Permission::Config],
                }],
            )
            .await
            .map(|_| ()),
        "user_status_update" => s
            .users
            .set_status(a, &f.bob.id, UserStatus::Disabled)
            .await
            .map(|_| ()),
        "user_reset_link" => s.users.reset_link(a, &f.bob.id).await.map(|_| ()),
        "user_delete" => s.users.delete(a, &f.bob.id).await,
        "whoami" => s.tokens.whoami(a).await.map(|_| ()),
        "audit_list" => s.audit.list(a, &Default::default()).await.map(|_| ()),
        "containers_list" => s.containers.list(a).await.map(|_| ()),
        "container_get" => s.containers.get(a, &f.cid).await.map(|_| ()),
        "container_logs" => s.containers.logs(a, &f.cid, None).await.map(|_| ()),
        "container_logs_stream" => s.containers.logs_stream(a, &f.cid, None).await.map(|_| ()),
        "container_start" => s.containers.start(a, &f.cid).await,
        "container_stop" => s.containers.stop(a, &f.cid).await,
        "container_restart" => s.containers.restart(a, &f.cid).await,
        "container_exec_session" => s.containers.exec_open(a, &f.cid, 80, 24).await.map(|_| ()),
        "system_usage" => s.system.usage(a).await.map(|_| ()),
        "system_logs" => s.system.logs(a, None).await.map(|_| ()),
        "system_logs_stream" => s.system.logs_stream(a, None).await.map(|_| ()),
        "system_cleanup_preview" => s.system.cleanup_preview(a).await.map(|_| ()),
        "system_cleanup" => s
            .system
            .cleanup(a, &CleanupSelection::defaults())
            .await
            .map(|_| ()),
        "health" => s.system.health(a).await.map(|_| ()),
        "cron_list" => s.cron.list(a).await.map(|_| ()),
        "cron_files_list" => s.cron.files_list(a).await.map(|_| ()),
        "cron_runs_list" => s.cron.runs_list(a, "crn_missing").await.map(|_| ()),
        "cron_preview" => s.cron.preview(a, "* * * * *", 5).await.map(|_| ()),
        "cron_create" => s.cron.create(a, &cron_input()).await.map(|_| ()),
        "cron_update" => s
            .cron
            .update(a, "crn_missing", &cron_input())
            .await
            .map(|_| ()),
        "cron_toggle" => s.cron.toggle(a, "crn_missing", false).await.map(|_| ()),
        "cron_delete" => s.cron.delete(a, "crn_missing").await,
        "cron_reapply" => s.cron.reapply(a, false).await,
        "cron_run" => s.cron.run_entry(a, "crn_missing").await.map(|_| ()),
        "authorized_keys_list" => s.authorized_keys.list(a).await.map(|_| ()),
        "authorized_keys_add" => s
            .authorized_keys
            .add(
                a,
                &AddKey {
                    name: "matrix".into(),
                    public_key: test_keys::ed25519(3, ""),
                },
            )
            .await
            .map(|_| ()),
        "authorized_keys_delete" => s
            .authorized_keys
            .delete(a, "akey_missing", None)
            .await
            .map(|_| ()),
        "backups_list" => s.backups.list(a).await.map(|_| ()),
        "backup_targets_list" => s.backups.targets(a).await.map(|_| ()),
        "backup_create" => s
            .backups
            .create_project(a, "prj_1", BackupKind::ProjectVolumes, None, false)
            .await
            .map(|_| ()),
        "backup_create_yanto" => s
            .backups
            .create_yanto(a, Trigger::Manual, None)
            .await
            .map(|_| ()),
        "backup_download" => s.backups.download(a, &f.project_backup).await.map(|_| ()),
        "backup_download_yanto" => s.backups.download(a, &f.yanto_backup).await.map(|_| ()),
        "backup_delete" => s.backups.delete(a, &f.project_backup).await,
        "backup_delete_yanto" => s.backups.delete(a, &f.yanto_backup).await,
        "backup_restore" => s
            .backups
            .restore(a, "bak_missing", Default::default())
            .await
            .map(|_| ()),
        "backup_prune" => s.backups.prune(a, Retention::default()).await.map(|_| ()),
        other if is_m6(other) => invoke_m6(f, other, a).await,
        other => match invoke_m2(f, other, a).await {
            Some(r) => r,
            None => match super::matrix_ingress::invoke(&f.core, other, a).await {
                Some(r) => r,
                None => panic!("no invocation for {other}"),
            },
        },
    }
}

#[tokio::test]
async fn every_actor_profile_against_every_operation() {
    let mut failures = Vec::new();
    for op in all_operations() {
        let row = grid_row(op.name);
        for (i, profile) in PROFILES.iter().enumerate() {
            let allowed = row[i];
            let f = fixture().await;
            let actor = actor_for(&f, profile).await;

            // 1. The spec-level decision. Member/token rows have no Project here, so the
            //    spec check uses an unrestricted call; Project-scoped ops arrive in M2.
            let spec = actor.authorize(op, project_for(op));
            if allowed {
                assert!(spec.is_ok(), "{} should allow {profile}: {spec:?}", op.name);
            } else {
                match spec {
                    Err(e) if e.kind == ErrorKind::Forbidden => {}
                    other => failures.push(format!(
                        "{} should deny {profile} with 403, got {other:?}",
                        op.name
                    )),
                }
            }

            // 2. The real service function must agree.
            let res = invoke(&f, op.name, &actor).await;
            match (allowed, res) {
                (false, Err(e)) if e.kind == ErrorKind::Forbidden => {}
                (false, other) => failures.push(format!(
                    "service {} let {profile} through: {other:?}",
                    op.name
                )),
                (true, Err(e))
                    if matches!(e.kind, ErrorKind::Forbidden | ErrorKind::Unauthenticated)
                        && op.access != crate::actor::Access::Public =>
                {
                    failures.push(format!("service {} wrongly denied {profile}: {e}", op.name));
                }
                (true, _) => {}
            }
        }
    }
    assert!(
        failures.is_empty(),
        "matrix violations:\n{}",
        failures.join("\n")
    );
}

#[tokio::test]
async fn happy_paths_really_succeed_for_the_owner() {
    // Guards against a matrix that "passes" only because every call fails for another reason.
    let f = fixture().await;
    let o = f.core.session_actor(&f.owner).await;
    let s = &f.core.services;
    s.users.list(&o).await.unwrap();
    s.users.get(&o, &f.bob.id).await.unwrap();
    s.users.create(&o, "newbie", &[]).await.unwrap();
    s.users
        .update_access(
            &o,
            &f.bob.id,
            &[GrantInput {
                project_id: "prj_9".into(),
                permissions: vec![Permission::Config],
            }],
        )
        .await
        .unwrap();
    s.users.reset_link(&o, &f.bob.id).await.unwrap();
    s.users
        .set_status(&o, &f.bob.id, UserStatus::Disabled)
        .await
        .unwrap();
    s.users.delete(&o, &f.bob.id).await.unwrap();
    s.tokens.create(&o, "m", Scope::Admin, None).await.unwrap();
    s.tokens.list(&o).await.unwrap();
    s.tokens.whoami(&o).await.unwrap();
    s.audit.list(&o, &Default::default()).await.unwrap();
    s.auth.owner_reset(&Actor::cli()).await.unwrap();
    // M5: the Owner can really use the host-management operations.
    let e = s.cron.create(&o, &cron_input()).await.unwrap();
    s.cron.toggle(&o, &e.id, false).await.unwrap();
    s.cron.run_entry(&o, &e.id).await.unwrap();
    s.cron.runs_list(&o, &e.id).await.unwrap();
    s.cron.delete(&o, &e.id).await.unwrap();
    let key = AddKey {
        name: "m".into(),
        public_key: test_keys::ed25519(4, ""),
    };
    let k = s.authorized_keys.add(&o, &key).await.unwrap();
    s.authorized_keys.list(&o).await.unwrap();
    s.authorized_keys
        .delete(&o, &k.key.id, Some("deploy"))
        .await
        .unwrap();
    s.backups.download(&o, &f.project_backup).await.unwrap();
    s.backups.delete(&o, &f.yanto_backup).await.unwrap();
}

fn rand_slug() -> String {
    format!("mx{}", crate::crypto::random_hex::<4>())
}

/// M2 operations. `None` when `op` is not one of them.
async fn invoke_m2(f: &Fixture, op: &str, a: &Actor) -> Option<Result<(), CoreError>> {
    use crate::service::deployments::{DeployRequest, GithubDelivery, RollbackRequest};
    use crate::service::projects::{DeleteOptions, ProjectInput, ProjectPatch, SourceInput};
    let s = &f.core.services;
    let origin =
        crate::actor::Origin::new(crate::actor::Channel::Webhook, Some("203.0.113.9".into()));
    let (p, d) = ("prj_1", f.dep.as_str());
    let r = match op {
        "project_list" => s.projects.list(a).await.map(|_| ()),
        "project_get" => s.projects.get(a, p).await.map(|_| ()),
        "project_create" => s
            .projects
            .create(
                a,
                &ProjectInput {
                    name: "Matrix".into(),
                    slug: Some(rand_slug()),
                    source: SourceInput::Image {
                        image_ref: "nginx".into(),
                    },
                    compose_path: None,
                    compose_content: None,
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
            .map(|_| ()),
        "project_update" => s
            .projects
            .update(
                a,
                p,
                &ProjectPatch {
                    name: Some("Renamed".into()),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ()),
        "project_delete" => s.projects.delete(a, p, &DeleteOptions::default()).await,
        "project_compose_read" => s.projects.compose_read(a, p).await.map(|_| ()),
        "project_env_read" => s.projects.env_read(a, p).await.map(|_| ()),
        "project_env_content_read" => s.projects.env_content_read(a, p, false).await.map(|_| ()),
        "project_env_preview" => s.projects.env_preview(a, None, "A=1").await.map(|_| ()),
        "project_env_write" => s.projects.env_write(a, p, "A=1\n").await.map(|_| ()),
        "project_env_variables_write" => s
            .projects
            .env_variables_write(
                a,
                p,
                &[crate::deploy::env::VarChange {
                    key: "A".into(),
                    value: Some("1".into()),
                }],
            )
            .await
            .map(|_| ()),
        "project_ports_list" => s.projects.ports_list(a, p).await.map(|_| ()),
        "project_ports_set" => s.projects.ports_set(a, p, &[]).await.map(|_| ()),
        "project_databases_list" => s.projects.databases_list(a, p).await.map(|_| ()),
        "project_databases_set" => s.projects.databases_set(a, p, &[]).await.map(|_| ()),
        "project_deploy_token_reveal" => s.projects.deploy_token_reveal(a, p).await.map(|_| ()),
        "project_deploy_token_rotate" => s.projects.deploy_token_rotate(a, p).await.map(|_| ()),
        "project_stop" => s.projects.stop(a, p).await.map(|_| ()),
        "project_restart" => s.projects.restart(a, p).await.map(|_| ()),
        "project_deploy" => s
            .deployments
            .deploy(a, p, &DeployRequest::default())
            .await
            .map(|_| ()),
        "project_rollback_preview" => s
            .deployments
            .rollback_preview(a, p, &RollbackRequest::default())
            .await
            .map(|_| ()),
        "project_rollback" => s
            .deployments
            .rollback(a, p, &RollbackRequest::default())
            .await
            .map(|_| ()),
        "project_reset_checkout" => s.deployments.reset_checkout(a, p).await.map(|_| ()),
        "deployments_list" => s.deployments.list(a, &Default::default()).await.map(|_| ()),
        "deployment_get" => s.deployments.get(a, d).await.map(|_| ()),
        "deployment_logs" => s.deployments.logs(a, d, None).await.map(|_| ()),
        "deployment_logs_stream" => s.deployments.log_stream(a, d, None).await.map(|_| ()),
        "deployment_cancel" => s.deployments.cancel(a, d).await.map(|_| ()),
        "deployments_events_stream" => s.deployments.events_stream(a).await.map(|_| ()),
        "deploy_webhook" => s
            .deployments
            .deploy_webhook(&origin, p, "ydp_nope")
            .await
            .map(|_| ()),
        "github_webhook" => s
            .deployments
            .github_webhook(
                &origin,
                p,
                &GithubDelivery {
                    signature: None,
                    event: Some("push"),
                    delivery_id: None,
                    form_encoded: false,
                    body: b"{}",
                },
            )
            .await
            .map(|_| ()),
        "deploy_key_list" => s.deploy_keys.list(a).await.map(|_| ()),
        "deploy_key_public_get" => s.deploy_keys.public_get(a, &f.key).await.map(|_| ()),
        "deploy_key_create" => s.deploy_keys.create(a, "matrix-key").await.map(|_| ()),
        "deploy_key_import" => s
            .deploy_keys
            .import(
                a,
                "matrix-import",
                &crate::secrets::Secret::new("not a key"),
            )
            .await
            .map(|_| ()),
        "deploy_key_delete" => s.deploy_keys.delete(a, &f.key).await,
        "registry_credential_list" => s.registry_credentials.list(a).await.map(|_| ()),
        "registry_credential_create" => s
            .registry_credentials
            .create(
                a,
                "matrix-cred",
                "docker.io",
                "u",
                &crate::secrets::Secret::new("pw-1234"),
            )
            .await
            .map(|_| ()),
        "registry_credential_update" => s
            .registry_credentials
            .update(a, &f.cred, &Default::default())
            .await
            .map(|_| ()),
        "registry_credential_delete" => s.registry_credentials.delete(a, &f.cred).await,
        _ => return None,
    };
    Some(r)
}

/// The per-permission cells of 08-conventions section 5 for every Project-scoped operation:
/// exact permission allows, a different permission is 403 (view-only ops allow), no access row
/// is 404, and a token never widens its user (scope caps; another Member's token is 404).
#[tokio::test]
async fn project_scoped_operations_per_permission() {
    use crate::actor::{Access, Need};
    let mut failures = Vec::new();
    let project_ops: Vec<_> = all_operations()
        .into_iter()
        .filter(|o| matches!(o.access, Access::Project(_)))
        .collect();
    assert!(project_ops.len() >= 15);
    for op in project_ops {
        let Access::Project(need) = op.access else {
            continue;
        };
        let mut f = fixture().await;
        let other_perms = |skip: Option<Permission>| -> Vec<Permission> {
            Permission::ALL
                .into_iter()
                .filter(|p| Some(*p) != skip)
                .collect()
        };
        let exact_perms: Vec<Permission> = match need {
            Need::Permission(p) => vec![p],
            Need::View => vec![],
        };
        let skip = match need {
            Need::Permission(p) => Some(p),
            Need::View => None,
        };
        let exact = f
            .core
            .create_member("exact", &[("prj_1", &exact_perms)])
            .await;
        let others = f
            .core
            .create_member("others", &[("prj_1", &other_perms(skip))])
            .await;
        let none = f.core.create_member("noaccess", &[("prj_2", &[])]).await;

        let kind_of = |r: Result<(), CoreError>| r.err().map(|e| e.kind);
        let check = |label: &str,
                     got: Option<ErrorKind>,
                     want: Option<ErrorKind>,
                     out: &mut Vec<String>| {
            if got.is_some() != want.is_some() || (want.is_some() && got != want) {
                out.push(format!(
                    "{} / {label}: expected {want:?}, got {got:?}",
                    op.name
                ));
            }
        };
        // Session actors: authorize() and the real service call must agree.
        let exact_s = f.core.session_actor(&exact).await;
        check(
            "exact permission",
            kind_of(exact_s.authorize(op, Some("prj_1"))),
            None,
            &mut failures,
        );
        let res = invoke(&f, op.name, &exact_s).await;
        if matches!(res, Err(ref e) if matches!(e.kind, ErrorKind::Forbidden | ErrorKind::NotFound))
        {
            failures.push(format!(
                "{} service denied the exact-permission member: {res:?}",
                op.name
            ));
        }
        // A successful delete consumed the seeded row; the Backup rule authorizes after loading
        // it (the Project comes from the row), so seed a fresh one for the remaining actors.
        if op.name == "backup_delete" {
            f.project_backup = project_backup_fixture(&f.core, "prj_1").await;
        }
        let others_s = f.core.session_actor(&others).await;
        let want_other = matches!(need, Need::Permission(_)).then_some(ErrorKind::Forbidden);
        check(
            "other permissions",
            kind_of(others_s.authorize(op, Some("prj_1"))),
            want_other.clone(),
            &mut failures,
        );
        if want_other.is_some() {
            let res = invoke(&f, op.name, &others_s).await;
            if !matches!(res, Err(ref e) if e.kind == ErrorKind::Forbidden) {
                failures.push(format!(
                    "{} service let a member with other permissions through: {res:?}",
                    op.name
                ));
            }
        }
        let none_s = f.core.session_actor(&none).await;
        check(
            "no access row",
            kind_of(none_s.authorize(op, Some("prj_1"))),
            Some(ErrorKind::NotFound),
            &mut failures,
        );
        let res = invoke(&f, op.name, &none_s).await;
        if !matches!(res, Err(ref e) if e.kind == ErrorKind::NotFound) {
            failures.push(format!(
                "{} service did not 404 a member without access: {res:?}",
                op.name
            ));
        }
        // Tokens: scope caps the user, never widens it.
        for scope in Scope::ALL {
            let (_, tok) = f.core.token_actor(&exact, scope).await;
            let want = (scope < op.min_scope).then_some(ErrorKind::Forbidden);
            check(
                &format!("exact member token {scope}"),
                kind_of(tok.authorize(op, Some("prj_1"))),
                want,
                &mut failures,
            );
            let (_, tok) = f.core.token_actor(&none, scope).await;
            let want = if scope < op.min_scope {
                ErrorKind::Forbidden
            } else {
                ErrorKind::NotFound
            };
            check(
                &format!("no-access member token {scope}"),
                kind_of(tok.authorize(op, Some("prj_1"))),
                Some(want),
                &mut failures,
            );
        }
    }
    assert!(failures.is_empty(), "violations:\n{}", failures.join("\n"));
}

fn is_m6(op: &str) -> bool {
    op.starts_with("registry_") && !op.starts_with("registry_credential")
}

/// M6 Hosted Registry operations (the registry is disabled in the fixture, so authorized
/// callers get a 409/404 from the service rather than a 403).
async fn invoke_m6(f: &Fixture, op: &str, a: &Actor) -> Result<(), CoreError> {
    use crate::service::registry::EnableInput;
    let r = &f.core.services.registry;
    match op {
        "registry_status" => r.status(a).await.map(|_| ()),
        "registry_users_list" => r.users_list(a).await.map(|_| ()),
        "registry_repos_list" => r.repos_list(a, None, None).await.map(|_| ()),
        "registry_tags_list" => r.tags_list(a, "app", None, None).await.map(|_| ()),
        "registry_tag_delete_preview" => r.tag_delete_preview(a, "app", "v1").await.map(|_| ()),
        "registry_gc_status" => r.gc_status(a).await.map(|_| ()),
        "registry_user_create" => r.user_create(a, "matrix", None).await.map(|_| ()),
        "registry_user_password_set" => r
            .user_password_set(a, "rus_missing", None)
            .await
            .map(|_| ()),
        "registry_user_delete" => r.user_delete(a, "rus_missing").await,
        "registry_enable" => r.enable(a, &EnableInput::default()).await.map(|_| ()),
        "registry_disable" => r.disable(a, None).await.map(|_| ()),
        "registry_settings_update" => r
            .settings_update(a, Some("registry.example.com"))
            .await
            .map(|_| ()),
        "registry_restart" => r.restart(a).await.map(|_| ()),
        "registry_rerender" => r.rerender(a).await.map(|_| ()),
        "registry_tag_delete" => r.tag_delete(a, "app", "v1").await.map(|_| ()),
        "registry_gc_preview" => r.gc_preview(a, false).await.map(|_| ()),
        "registry_gc" => r.gc_start(a, false).await.map(|_| ()),
        other => panic!("no invocation for {other}"),
    }
}
