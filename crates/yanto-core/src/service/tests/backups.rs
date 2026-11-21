//! Backups: the Yanto-DB archive, framework behavior, authorization, retention, restore.

use std::time::Duration;

use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::{Actor, Permission};
use crate::backup::{self, BackupKind, Trigger, Verify};
use crate::db::repo::backups::{self as repo, NewBackup};
use crate::service::backups::{BUSY_MESSAGE, Retention};
use crate::testing::TestCore;

fn with_key(core: &TestCore) -> [u8; 32] {
    let key = crate::crypto::random_bytes::<32>();
    let dir = core.dir.path().join("keys");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("app.key"), key).unwrap();
    key
}

async fn yanto_backup(core: &TestCore, trigger: Trigger) -> crate::service::backups::BackupDto {
    core.clock.advance(Duration::from_secs(1));
    core.services
        .backups
        .create_yanto(&Actor::cli(), trigger, None)
        .await
        .unwrap()
}

#[tokio::test]
async fn yanto_backup_bundles_a_checked_db_and_the_key() {
    let core = TestCore::new().await;
    let key = with_key(&core);
    core.setup_owner().await;
    let b = yanto_backup(&core, Trigger::Manual).await;
    assert_eq!(
        (b.status.as_str(), b.kind.as_str(), b.sensitive),
        ("success", "yanto_sqlite", true)
    );
    assert!(b.filename.ends_with("-yanto.tar.zst"));
    let path = core.dir.path().join("backups/yanto").join(&b.filename);
    assert_eq!(
        backup::sha256_file(&path).unwrap(),
        b.sha256.clone().unwrap()
    );
    assert_eq!(
        b.size_bytes,
        Some(std::fs::metadata(&path).unwrap().len() as i64)
    );
    assert!(backup::sidecar_path(&path).exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&core.dir.path().join("backups")), 0o700);
        assert_eq!(mode(&core.dir.path().join("backups/yanto")), 0o700);
    }
    // No intermediate files are left behind.
    let leftovers: Vec<_> = std::fs::read_dir(core.dir.path().join("backups/yanto"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    let out = core.dir.path().join("x");
    backup::ensure_private_dir(&out).unwrap();
    let x = backup::extract_yanto_archive(&path, &out).unwrap();
    assert_eq!(std::fs::read(x.key).unwrap(), key);
    backup::integrity_check(&x.db).await.unwrap();
    let audit: String =
        sqlx::query_scalar("SELECT metadata FROM audit_logs WHERE action = 'backup.create'")
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert!(audit.contains("\"sensitive\":true") && !audit.contains(&crate::crypto::hex(&key)));
}

#[tokio::test]
async fn snapshot_is_consistent_under_concurrent_writes() {
    let core = TestCore::new().await;
    with_key(&core);
    core.setup_owner().await;
    let db = core.db.clone();
    let writer = tokio::spawn(async move {
        for i in 0..300 {
            let mut tx = db.begin_write().await.unwrap();
            sqlx::query("INSERT INTO app_settings (key, value, updated_at) VALUES (?, '1', 0)")
                .bind(format!("load.{i}"))
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    });
    let b = yanto_backup(&core, Trigger::Manual).await;
    writer.await.unwrap();
    let path = core.dir.path().join("backups/yanto").join(&b.filename);
    let out = core.dir.path().join("x");
    backup::ensure_private_dir(&out).unwrap();
    let x = backup::extract_yanto_archive(&path, &out).unwrap();
    backup::integrity_check(&x.db).await.unwrap();
}

#[tokio::test]
async fn missing_key_or_low_space_or_busy_lock_refuse_without_a_row() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let s = &core.services.backups;
    assert!(matches!(
        kind(s.create_yanto(&a, Trigger::Manual, None).await),
        ErrorKind::Validation(_)
    ));
    with_key(&core);
    core.host.backup_host.set_free_bytes(Some(1024));
    assert_eq!(
        kind(s.create_yanto(&a, Trigger::Manual, None).await),
        ErrorKind::Unavailable
    );
    core.host.backup_host.set_free_bytes(None);
    let _held = crate::ports::BackupHost::try_lock(&*core.host.backup_host, "backup-yanto")
        .unwrap()
        .unwrap();
    let e = s.create_yanto(&a, Trigger::Manual, None).await.unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (ErrorKind::Conflict, BUSY_MESSAGE)
    );
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backups")
        .fetch_one(core.db.reader())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn free_space_guard_doubles_the_previous_backup_size() {
    let core = TestCore::new().await;
    with_key(&core);
    let b = yanto_backup(&core, Trigger::Manual).await;
    assert!(b.size_bytes.unwrap() > 0);
    // Plenty above 1 GiB but below 2x a (pretend huge) previous backup.
    sqlx::query("UPDATE backups SET size_bytes = ?")
        .bind(4_i64 * 1024 * 1024 * 1024)
        .execute(core.db.writer())
        .await
        .unwrap();
    core.host
        .backup_host
        .set_free_bytes(Some(6 * 1024 * 1024 * 1024));
    assert_eq!(
        kind(
            core.services
                .backups
                .create_yanto(&Actor::cli(), Trigger::Manual, None)
                .await
        ),
        ErrorKind::Unavailable
    );
    core.host
        .backup_host
        .set_free_bytes(Some(9 * 1024 * 1024 * 1024));
    core.clock.advance(Duration::from_secs(1));
    core.services
        .backups
        .create_yanto(&Actor::cli(), Trigger::Manual, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn same_second_backups_get_distinct_names() {
    let core = TestCore::new().await;
    with_key(&core);
    let a = Actor::cli();
    let s = &core.services.backups;
    let one = s.create_yanto(&a, Trigger::Manual, None).await.unwrap();
    let two = s.create_yanto(&a, Trigger::Manual, None).await.unwrap();
    assert_ne!(one.filename, two.filename);
}

#[tokio::test]
async fn scheduled_runs_are_pruned_manual_ones_are_not() {
    let core = TestCore::new().await;
    with_key(&core);
    let a = Actor::cli();
    let manual = yanto_backup(&core, Trigger::Manual).await;
    let policy = Some(Retention {
        keep_last: Some(2),
        keep_days: None,
    });
    let mut last = None;
    for _ in 0..3 {
        core.clock.advance(Duration::from_secs(60));
        last = Some(
            core.services
                .backups
                .create_yanto(&a, Trigger::Cron, policy)
                .await
                .unwrap(),
        );
    }
    let list = core.services.backups.list(&a).await.unwrap();
    let cron: Vec<_> = list.iter().filter(|b| b.trigger == "cron").collect();
    assert_eq!(cron.len(), 2, "keep-last 2 leaves two scheduled backups");
    assert!(
        list.iter().any(|b| b.id == manual.id),
        "manual backups are untouched"
    );
    assert!(cron.iter().any(|b| b.id == last.as_ref().unwrap().id));
    let files = std::fs::read_dir(core.dir.path().join("backups/yanto"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tar.zst"))
        .count();
    assert_eq!(
        files, 3,
        "pruned archives and their sidecars are removed from disk"
    );
    assert_eq!(audit_count(&core, "backup.prune", "ok").await, 1);
}

#[tokio::test]
async fn keep_days_prunes_old_scheduled_backups_but_never_the_newest() {
    let core = TestCore::new().await;
    with_key(&core);
    let a = Actor::cli();
    let days = Some(Retention {
        keep_last: None,
        keep_days: Some(10),
    });
    core.services
        .backups
        .create_yanto(&a, Trigger::Cron, days)
        .await
        .unwrap();
    core.clock.advance(Duration::from_secs(20 * 86_400));
    core.services
        .backups
        .create_yanto(&a, Trigger::Cron, days)
        .await
        .unwrap();
    core.clock.advance(Duration::from_secs(40 * 86_400));
    // Both older ones go; the newest successful one is always kept, then the fresh one lands.
    core.services
        .backups
        .create_yanto(&a, Trigger::Cron, days)
        .await
        .unwrap();
    let list = core.services.backups.list(&a).await.unwrap();
    assert_eq!(list.len(), 2);
    // `prune` with the same flags is a no-op now and refuses tokens (CLI only).
    assert_eq!(
        core.services
            .backups
            .prune(&a, Retention::default())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn failed_backups_older_than_a_week_are_purged_by_the_next_scheduled_run() {
    let core = TestCore::new().await;
    with_key(&core);
    let a = Actor::cli();
    let mut conn = core.db.writer().acquire().await.unwrap();
    repo::insert_running(
        &mut conn,
        &NewBackup {
            id: "bak_old",
            project_id: None,
            kind: "yanto_sqlite",
            database_id: None,
            trigger: "manual",
            filename: "x",
            file_path: "/nonexistent",
            note: None,
            now: 0,
        },
    )
    .await
    .unwrap();
    repo::mark_failed(&mut conn, "bak_old", "boom", 1)
        .await
        .unwrap();
    drop(conn);
    core.services
        .backups
        .create_yanto(&a, Trigger::Cron, None)
        .await
        .unwrap();
    let list = core.services.backups.list(&a).await.unwrap();
    assert!(list.iter().all(|b| b.id != "bak_old"));
}

async fn project_backup_fixture(core: &TestCore, project: &str) -> String {
    let dir = core.dir.path().join("backups/projects/blog");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("20260101T000000Z-postgres.sql.zst");
    std::fs::write(&file, b"dump").unwrap();
    let mut conn = core.db.writer().acquire().await.unwrap();
    let id = "bak_proj";
    repo::insert_running(
        &mut conn,
        &NewBackup {
            id,
            project_id: Some(project),
            kind: "project_postgres",
            database_id: None,
            trigger: "manual",
            filename: "20260101T000000Z-postgres.sql.zst",
            file_path: &file.to_string_lossy(),
            note: None,
            now: 1,
        },
    )
    .await
    .unwrap();
    repo::mark_success(&mut conn, id, 4, "abc", 2)
        .await
        .unwrap();
    id.to_owned()
}

#[tokio::test]
async fn authorization_owner_only_for_yanto_backups_and_backups_permission_for_projects() {
    let core = TestCore::new().await;
    with_key(&core);
    core.setup_owner().await;
    let carol = core
        .create_member("carol", &[("prj_1", &[Permission::Backups])])
        .await;
    let dave = core
        .create_member("dave", &[("prj_1", &[Permission::Deploy])])
        .await;
    let erin = core.create_member("erin", &[]).await;
    let (carol, dave, erin) = (
        core.session_actor(&carol).await,
        core.session_actor(&dave).await,
        core.session_actor(&erin).await,
    );
    let s = &core.services.backups;
    let y = yanto_backup(&core, Trigger::Manual).await;
    let p = project_backup_fixture(&core, "prj_1").await;

    // The Yanto-DB archive: Owner only, even for a Member who holds `backups` on a Project.
    for who in [&carol, &dave, &erin] {
        assert_eq!(kind(s.download(who, &y.id).await), ErrorKind::Forbidden);
        assert_eq!(kind(s.delete(who, &y.id).await), ErrorKind::Forbidden);
        assert_eq!(
            kind(s.create_yanto(who, Trigger::Manual, None).await),
            ErrorKind::Forbidden
        );
        assert!(s.list(who).await.unwrap().iter().all(|b| b.id != y.id));
    }
    // A Project backup: needs `backups` on that Project.
    assert_eq!(s.list(&carol).await.unwrap().len(), 1);
    assert_eq!(kind(s.download(&dave, &p).await), ErrorKind::Forbidden);
    assert_eq!(kind(s.download(&erin, &p).await), ErrorKind::NotFound);
    assert_eq!(kind(s.delete(&erin, &p).await), ErrorKind::NotFound);
    assert!(s.list(&dave).await.unwrap().is_empty());
    let d = s.download(&carol, &p).await.unwrap();
    assert_eq!(
        (d.sha256.as_str(), d.size_bytes, d.sensitive),
        ("abc", 4, false)
    );
    assert_eq!(
        kind(s.download(&carol, "bak_missing").await),
        ErrorKind::NotFound
    );
    s.delete(&carol, &p).await.unwrap();
    assert!(
        !core
            .dir
            .path()
            .join("backups/projects/blog/20260101T000000Z-postgres.sql.zst")
            .exists()
    );
}

#[tokio::test]
async fn download_counts_audits_and_refuses_unfinished_or_escaping_paths() {
    let core = TestCore::new().await;
    with_key(&core);
    let a = Actor::cli();
    let b = yanto_backup(&core, Trigger::Manual).await;
    let d = core.services.backups.download(&a, &b.id).await.unwrap();
    assert!(d.sensitive && d.path.is_file());
    core.services.backups.download(&a, &b.id).await.unwrap();
    let row = core.services.backups.list(&a).await.unwrap().remove(0);
    assert_eq!(row.download_count, 2);
    assert!(row.downloaded_at.is_some());
    assert_eq!(audit_count(&core, "backup.download", "ok").await, 2);
    // A tampered row pointing outside backups/ is refused.
    let outside = core.dir.path().join("elsewhere");
    std::fs::write(&outside, b"x").unwrap();
    sqlx::query("UPDATE backups SET file_path = ?")
        .bind(outside.to_string_lossy().into_owned())
        .execute(core.db.writer())
        .await
        .unwrap();
    assert_eq!(
        kind(core.services.backups.download(&a, &b.id).await),
        ErrorKind::Internal
    );
    core.services.backups.delete(&a, &b.id).await.unwrap();
    assert!(
        outside.exists(),
        "delete never removes files outside backups/"
    );
    // Running rows can be neither downloaded nor deleted.
    let mut conn = core.db.writer().acquire().await.unwrap();
    repo::insert_running(
        &mut conn,
        &NewBackup {
            id: "bak_run",
            project_id: None,
            kind: "yanto_sqlite",
            database_id: None,
            trigger: "manual",
            filename: "f",
            file_path: "/x",
            note: None,
            now: 5,
        },
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        kind(core.services.backups.delete(&a, "bak_run").await),
        ErrorKind::Conflict
    );
    assert_eq!(
        kind(core.services.backups.download(&a, "bak_run").await),
        ErrorKind::Conflict
    );
}

#[tokio::test]
async fn interrupted_backups_are_recovered_on_startup() {
    let core = TestCore::new().await;
    with_key(&core);
    let dir = core.dir.path().join("backups/yanto");
    std::fs::create_dir_all(&dir).unwrap();
    let partial = dir.join(".x.tar.zst.partial");
    std::fs::write(&partial, b"half").unwrap();
    let mut conn = core.db.writer().acquire().await.unwrap();
    repo::insert_running(
        &mut conn,
        &NewBackup {
            id: "bak_cut",
            project_id: None,
            kind: "yanto_sqlite",
            database_id: None,
            trigger: "cron",
            filename: "x.tar.zst",
            file_path: &dir.join("x.tar.zst").to_string_lossy(),
            note: None,
            now: 1,
        },
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        core.services.backups.recover_interrupted().await.unwrap(),
        1
    );
    assert!(!partial.exists());
    let b = &core.services.backups.list(&Actor::cli()).await.unwrap()[0];
    assert_eq!(
        (b.status.as_str(), b.error.as_deref()),
        ("failed", Some("interrupted"))
    );
    assert_eq!(
        core.services.backups.recover_interrupted().await.unwrap(),
        0
    );
}

#[tokio::test]
async fn generic_project_framework_with_a_fake_port() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let s = &core.services.backups;
    let b = s
        .run_project_for_test(&a, "prj_1", "blog", BackupKind::ProjectPostgres, Some("db"))
        .await
        .unwrap();
    assert!(b.filename.ends_with("-postgres.sql.zst") && !b.sensitive);
    let req = core.host.project_backups.calls().pop().unwrap();
    assert_eq!(
        (req.project_slug.as_str(), req.target.as_deref()),
        ("blog", Some("db"))
    );
    let path = core
        .dir
        .path()
        .join("backups/projects/blog")
        .join(&b.filename);
    assert_eq!(std::fs::read(&path).unwrap(), b"fake-archive");
    assert_eq!(b.sha256.unwrap(), backup::sha256_file(&path).unwrap());
    // A failing port leaves a failed row and no files.
    core.host
        .project_backups
        .fail_next(crate::error::CoreError::new(
            ErrorKind::Unavailable,
            "container stopped",
        ));
    core.clock.advance(Duration::from_secs(1));
    let e = s
        .run_project_for_test(&a, "prj_1", "blog", BackupKind::ProjectPostgres, Some("db"))
        .await;
    assert_eq!(kind(e), ErrorKind::Unavailable);
    let list = s.list(&a).await.unwrap();
    let failed = list.iter().find(|b| b.status == "failed").unwrap();
    assert_eq!(failed.error.as_deref(), Some("container stopped"));
    let files: Vec<_> = std::fs::read_dir(core.dir.path().join("backups/projects/blog"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(
        files.len(),
        2,
        "one archive and its sidecar only: {files:?}"
    );
    // Slugs and targets that could escape the directory are refused.
    for (slug, target) in [("../x", None), ("ok", Some("a/b"))] {
        assert!(matches!(
            kind(
                s.run_project_for_test(&a, "prj_1", slug, BackupKind::ProjectVolumes, target)
                    .await
            ),
            ErrorKind::Validation(_)
        ));
    }
}

#[tokio::test]
async fn project_backups_are_unavailable_until_m2_merges() {
    let core = TestCore::new().await;
    let r = core
        .services
        .backups
        .create_project(
            &Actor::cli(),
            "prj_1",
            BackupKind::ProjectVolumes,
            None,
            false,
        )
        .await;
    assert_eq!(kind(r), ErrorKind::Unavailable);
    assert!(matches!(
        kind(
            core.services
                .backups
                .create_project(&Actor::cli(), "prj_1", BackupKind::YantoSqlite, None, false)
                .await
        ),
        ErrorKind::Validation(_)
    ));
}

#[tokio::test]
#[ignore = "TODO(M2-merge): needs Projects and project_databases to exist"]
async fn project_volume_and_database_backups_end_to_end() {
    // After M2 merges: create a Project with a Postgres service, then expect
    // `create_project(owner, id, ProjectPostgres, Some("db"), false)` to produce a `.sql.zst`
    // via the real ProjectBackupPort, `project_slug` to resolve the slug, retention per
    // (project, kind, target) series, and `backups` permission Members to list/download it.
    todo!("implement once M2 is merged");
}

#[tokio::test]
async fn restore_roundtrip_into_a_fresh_data_dir() {
    let core = TestCore::new().await;
    let key = with_key(&core);
    let owner = core.setup_owner().await;
    let b = yanto_backup(&core, Trigger::Manual).await;
    let archive = core.dir.path().join("backups/yanto").join(&b.filename);

    let fresh = tempfile::tempdir().unwrap();
    // Pre-existing state in the target dir must be moved aside, not destroyed.
    std::fs::write(fresh.path().join("yanto.db"), b"old database").unwrap();
    std::fs::create_dir_all(fresh.path().join("keys")).unwrap();
    std::fs::write(fresh.path().join("keys/app.key"), [9u8; 32]).unwrap();
    let r = backup::restore_yanto(fresh.path(), &archive, 1_767_225_600_000, Verify::Required)
        .await
        .unwrap();
    assert!(r.key_replaced);
    assert_eq!(
        std::fs::read(r.previous_db.unwrap()).unwrap(),
        b"old database"
    );
    assert_eq!(
        std::fs::read(r.previous_key.unwrap()).unwrap(),
        vec![9u8; 32]
    );
    assert_eq!(
        std::fs::read(fresh.path().join("keys/app.key")).unwrap(),
        key
    );
    let db = crate::Db::open(&fresh.path().join("yanto.db"))
        .await
        .unwrap();
    let name: String = sqlx::query_scalar("SELECT username FROM users WHERE role = 'owner'")
        .fetch_one(db.reader())
        .await
        .unwrap();
    assert_eq!(name, owner.username);
    assert!(
        !fresh
            .path()
            .join("tmp")
            .read_dir()
            .unwrap()
            .any(|e| e.is_ok()),
        "temp dir cleaned"
    );
}

#[tokio::test]
async fn restore_checks_the_checksum_before_changing_anything() {
    let core = TestCore::new().await;
    with_key(&core);
    let b = yanto_backup(&core, Trigger::Manual).await;
    let archive = core.dir.path().join("backups/yanto").join(&b.filename);
    let fresh = tempfile::tempdir().unwrap();
    std::fs::write(fresh.path().join("yanto.db"), b"keep me").unwrap();

    // Corrupt one byte: the sidecar no longer matches.
    let mut bytes = std::fs::read(&archive).unwrap();
    let n = bytes.len() / 2;
    bytes[n] ^= 0xff;
    let bad = fresh.path().join("bad.tar.zst");
    std::fs::write(&bad, &bytes).unwrap();
    std::fs::copy(backup::sidecar_path(&archive), backup::sidecar_path(&bad)).unwrap();
    let e = backup::restore_yanto(fresh.path(), &bad, 0, Verify::Required)
        .await
        .unwrap_err();
    assert!(e.message.contains("checksum mismatch"), "{}", e.message);
    assert_eq!(
        std::fs::read(fresh.path().join("yanto.db")).unwrap(),
        b"keep me"
    );

    // No sidecar: refused unless the operator opts out.
    let lone = fresh.path().join("lone.tar.zst");
    std::fs::copy(&archive, &lone).unwrap();
    assert!(
        backup::restore_yanto(fresh.path(), &lone, 0, Verify::Required)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(fresh.path().join("yanto.db")).unwrap(),
        b"keep me"
    );
    backup::restore_yanto(fresh.path(), &lone, 0, Verify::IfPresent)
        .await
        .unwrap();
}
