//! Authorized Keys: block surgery through the service, lockout guard, audit.

use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::Actor;
use crate::authkeys::test_keys::{ed25519, rsa};
use crate::authkeys::{BEGIN_MARKER, END_MARKER, count_valid_keys};
use crate::service::authorized_keys::{AddKey, FileState};
use crate::testing::TestCore;

fn add(name: &str, key: String) -> AddKey {
    AddKey {
        name: name.into(),
        public_key: key,
    }
}

fn file(core: &TestCore) -> String {
    String::from_utf8(core.host.authorized_keys.content().expect("file exists")).unwrap()
}

#[tokio::test]
async fn add_creates_the_block_and_audits_without_the_key_body() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let key = ed25519(1, "laptop");
    let r = core
        .services
        .authorized_keys
        .add(&a, &add("laptop", key.clone()))
        .await
        .unwrap();
    let text = file(&core);
    assert!(text.starts_with(BEGIN_MARKER) && text.ends_with(&format!("{END_MARKER}\n")));
    let body = key.split_whitespace().nth(1).unwrap();
    assert!(text.contains(&format!("ssh-ed25519 {body} yanto:{}:laptop\n", r.key.id)));
    let meta: String =
        sqlx::query_scalar("SELECT metadata FROM audit_logs WHERE action = 'authorized_key.add'")
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert!(meta.contains("SHA256:") && !meta.contains(body));
}

#[tokio::test]
async fn foreign_lines_survive_every_change_byte_for_byte() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let foreign = format!(
        "{}\r\n# kept\r\ncommand=\"x y\" {}",
        ed25519(9, "manual"),
        ed25519(8, "ci")
    );
    core.host
        .authorized_keys
        .set_content(Some(foreign.clone().into_bytes()));
    let k1 = core
        .services
        .authorized_keys
        .add(&a, &add("one", ed25519(1, "")))
        .await
        .unwrap();
    let k2 = core
        .services
        .authorized_keys
        .add(&a, &add("two", ed25519(2, "")))
        .await
        .unwrap();
    let text = file(&core);
    assert!(text.starts_with(&foreign), "outside bytes preserved");
    core.services
        .authorized_keys
        .delete(&a, &k1.key.id, None)
        .await
        .unwrap();
    core.services
        .authorized_keys
        .delete(&a, &k2.key.id, None)
        .await
        .unwrap();
    let after = file(&core);
    // Foreign content plus the one newline the first append had to add, then an empty block.
    assert!(after.starts_with(&foreign));
    assert_eq!(count_valid_keys(after.as_bytes()), 2);
}

#[tokio::test]
async fn invalid_and_duplicate_keys_are_rejected_without_writing() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let svc = &core.services.authorized_keys;
    for bad in [
        format!("command=\"id\" {}", ed25519(1, "")),
        "ssh-dss AAAA".to_owned(),
        rsa(1024, ""),
        format!("{}\n{}", ed25519(1, ""), ed25519(2, "")),
    ] {
        assert!(matches!(
            kind(svc.add(&a, &add("x", bad)).await),
            ErrorKind::Validation(_)
        ));
    }
    assert_eq!(core.host.authorized_keys.write_count(), 0);
    svc.add(&a, &add("one", ed25519(1, ""))).await.unwrap();
    // Same key again (different comment/name) is a conflict and changes nothing.
    assert_eq!(
        kind(svc.add(&a, &add("two", ed25519(1, "other"))).await),
        ErrorKind::Conflict
    );
    assert_eq!(core.host.authorized_keys.write_count(), 1);
}

#[tokio::test]
async fn ambiguous_markers_refuse_with_no_write_and_no_row() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let broken = format!("{}\n{BEGIN_MARKER}\nstuff\n", ed25519(9, "m"));
    core.host
        .authorized_keys
        .set_content(Some(broken.clone().into_bytes()));
    let r = core
        .services
        .authorized_keys
        .add(&a, &add("x", ed25519(1, "")))
        .await;
    assert_eq!(kind(r), ErrorKind::Conflict);
    assert_eq!(file(&core), broken);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authorized_keys")
        .fetch_one(core.db.reader())
        .await
        .unwrap();
    assert_eq!(n, 0);
    let o = core.services.authorized_keys.list(&a).await.unwrap();
    assert!(matches!(o.file, FileState::Ambiguous { .. }));
}

#[tokio::test]
async fn lockout_guard_blocks_removing_the_last_key_unless_outside_keys_exist() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let svc = &core.services.authorized_keys;
    let k = svc.add(&a, &add("only", ed25519(1, ""))).await.unwrap();
    // Rule 1: nothing else in the file, so removal is refused and the DB row stays.
    let before = file(&core);
    assert_eq!(
        kind(svc.delete(&a, &k.key.id, None).await),
        ErrorKind::Conflict
    );
    assert_eq!(
        kind(svc.delete(&a, &k.key.id, Some("wrong")).await),
        ErrorKind::Conflict
    );
    assert_eq!(file(&core), before);
    assert_eq!(svc.list(&a).await.unwrap().keys.len(), 1);
    // Override with the typed Unix user name is audited as force_change.
    let d = svc.delete(&a, &k.key.id, Some("deploy")).await.unwrap();
    assert!(d.forced && d.remaining_keys == 0);
    assert_eq!(
        audit_count(&core, "authorized_key.force_change", "ok").await,
        1
    );

    // With a key outside the block the same removal is a plain delete.
    let core = TestCore::new().await;
    core.host
        .authorized_keys
        .set_content(Some(format!("{}\n", ed25519(9, "manual")).into_bytes()));
    let svc = &core.services.authorized_keys;
    let k = svc.add(&a, &add("only", ed25519(1, ""))).await.unwrap();
    let d = svc.delete(&a, &k.key.id, None).await.unwrap();
    assert!(!d.forced && d.remaining_keys == 1);
    assert_eq!(audit_count(&core, "authorized_key.delete", "ok").await, 1);
    assert_eq!(
        kind(svc.delete(&a, "akey_none", None).await),
        ErrorKind::NotFound
    );
}

#[tokio::test]
async fn duplicate_outside_the_block_warns_but_succeeds() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    core.host
        .authorized_keys
        .set_content(Some(format!("{}\n", ed25519(1, "manual")).into_bytes()));
    let r = core
        .services
        .authorized_keys
        .add(&a, &add("dup", ed25519(1, "")))
        .await
        .unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("outside")));
}

#[tokio::test]
async fn host_failure_rolls_the_row_back() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    core.host
        .authorized_keys
        .fail_next(crate::error::CoreError::new(
            ErrorKind::Unavailable,
            "disk full",
        ));
    assert_eq!(
        kind(
            core.services
                .authorized_keys
                .add(&a, &add("x", ed25519(1, "")))
                .await
        ),
        ErrorKind::Unavailable
    );
    assert!(
        core.services
            .authorized_keys
            .list(&a)
            .await
            .unwrap()
            .keys
            .is_empty()
    );
    assert_eq!(audit_count(&core, "authorized_key.add", "ok").await, 0);
}

#[tokio::test]
async fn list_reports_state_outside_keys_and_missing_user() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let o = core.services.authorized_keys.list(&a).await.unwrap();
    assert_eq!(o.file, FileState::Missing);
    assert!(o.block_in_sync && o.problem.is_none());
    core.host
        .authorized_keys
        .set_content(Some(format!("{}\n", ed25519(9, "m")).into_bytes()));
    core.services
        .authorized_keys
        .add(&a, &add("one", ed25519(1, "")))
        .await
        .unwrap();
    let o = core.services.authorized_keys.list(&a).await.unwrap();
    assert_eq!(
        (o.outside_keys, o.keys.len(), o.block_in_sync),
        (1, 1, true)
    );
    // Someone edits inside the block by hand: reported as out of sync.
    let edited = file(&core).replace("yanto:", "yanto-x:");
    core.host
        .authorized_keys
        .set_content(Some(edited.into_bytes()));
    assert!(
        !core
            .services
            .authorized_keys
            .list(&a)
            .await
            .unwrap()
            .block_in_sync
    );
    // A different, non-existent configured user disables the feature.
    let mut cfg = core.config.clone();
    cfg.ssh.authorized_keys_user = "ghost".into();
    let svc = crate::service::Services::new(crate::service::ServiceDeps {
        db: core.db.clone(),
        config: &cfg,
        clock: core.clock.clone(),
        secrets: std::sync::Arc::new(crate::secrets::Secrets::ephemeral()),
        passwords: crate::password::Passwords::cheap_for_tests(),
        docker: core.docker.clone(),
        host_stats: core.host_stats.clone(),
        host_mgmt: core.host.ports(),
        host: crate::deploy::HostPorts {
            command: core.commands.clone(),
            probe: core.probe.clone(),
            registry_auth: core.registry_auth.clone(),
        },
        registry: crate::ports::RegistryDeps::new(core.registry_api.clone()),
    });
    let o = svc.authorized_keys.list(&a).await.unwrap();
    assert!(o.problem.is_some());
    assert!(matches!(
        kind(svc.authorized_keys.add(&a, &add("x", ed25519(5, ""))).await),
        ErrorKind::Validation(_)
    ));
}
