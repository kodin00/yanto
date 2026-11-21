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
use crate::actor::{Access, Actor, Permission, Scope};
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
    ("backup_prune", "Y   N    N   N   N   N   N   N   Y"),
];

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
    let owner = core.setup_owner().await;
    let alice = core
        .create_member("alice", &[("prj_1", &[Permission::Deploy])])
        .await;
    let bob = core.create_member("bob", &[]).await;
    let cid = core.docker.add_container("web", "running", &[]);
    let project_backup = project_backup_fixture(&core, "prj_1").await;
    let yanto_backup = yanto_row_fixture(&core).await;
    Fixture {
        core,
        owner,
        alice,
        bob,
        cid,
        project_backup,
        yanto_backup,
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
        "backup_prune" => s.backups.prune(a, Retention::default()).await.map(|_| ()),
        other => panic!("no invocation for {other}"),
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
            let project = matches!(op.access, Access::Project(_)).then_some("prj_1");
            let spec = actor.authorize(op, project);
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
