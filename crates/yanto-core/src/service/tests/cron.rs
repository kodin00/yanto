//! Cron Entries: file generation, rollback, drift, and `cron-run` history.

use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::Actor;
use crate::cron::{EntryInput, Overlap};
use crate::error::CoreError;
use crate::ports::{ExecOutcome, ManagedFile, UnixUser};
use crate::service::cron::FileStatus;
use crate::testing::TestCore;

fn input(name: &str) -> EntryInput {
    EntryInput {
        name: name.into(),
        schedule: "*/5 * * * *".into(),
        run_as_user: "deploy".into(),
        command: "echo 100% done".into(),
        overlap: Overlap::Allow,
        timeout_s: None,
        enabled: true,
    }
}

async fn entry_count(core: &TestCore) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM cron_entries")
        .fetch_one(core.db.reader())
        .await
        .unwrap()
}

#[tokio::test]
async fn create_renders_a_wrapped_line_and_tracks_the_file() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let e = core
        .services
        .cron
        .create(&a, &input("nightly"))
        .await
        .unwrap();
    let text = core.host.cron.content().expect("file written");
    assert!(text.ends_with('\n'));
    assert!(text.contains(&format!(
        "*/5 * * * * root /usr/local/bin/yanto cron-run {}\n",
        e.id
    )));
    // The command (with its %) never reaches the file.
    assert!(!text.contains("echo") && !text.contains("100%"));
    assert_eq!(
        core.services.cron.list(&a).await.unwrap().file,
        FileStatus::InSync
    );
    assert_eq!(audit_count(&core, "cron.create", "ok").await, 1);
    let meta: String =
        sqlx::query_scalar("SELECT metadata FROM audit_logs WHERE action = 'cron.create'")
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert!(meta.contains("echo 100% done"), "full command is audited");
}

#[tokio::test]
async fn invalid_input_writes_nothing() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let mut bad = input("x");
    bad.schedule = "61 * * * *".into();
    assert!(matches!(
        kind(core.services.cron.create(&a, &bad).await),
        ErrorKind::Validation(f) if f[0].field == "schedule"
    ));
    let mut ghost = input("y");
    ghost.run_as_user = "nobody-such".into();
    assert!(matches!(
        kind(core.services.cron.create(&a, &ghost).await),
        ErrorKind::Validation(f) if f[0].field == "run_as_user"
    ));
    assert!(core.host.cron.writes().is_empty());
    assert_eq!(entry_count(&core).await, 0);
}

#[tokio::test]
async fn duplicate_name_conflicts_and_file_is_unchanged() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    core.services.cron.create(&a, &input("dup")).await.unwrap();
    let before = core.host.cron.writes().len();
    assert_eq!(
        kind(core.services.cron.create(&a, &input("dup")).await),
        ErrorKind::Conflict
    );
    assert_eq!(core.host.cron.writes().len(), before);
}

#[tokio::test]
async fn failed_write_rolls_the_entry_back() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    core.host.cron.fail_next(CoreError::new(
        ErrorKind::Unavailable,
        "read-only file system",
    ));
    let r = core.services.cron.create(&a, &input("x")).await;
    assert_eq!(kind(r), ErrorKind::Unavailable);
    assert_eq!(entry_count(&core).await, 0);
    assert_eq!(audit_count(&core, "cron.create", "ok").await, 0);
    // The service recovers on the next attempt.
    core.services.cron.create(&a, &input("x")).await.unwrap();
    assert_eq!(entry_count(&core).await, 1);
}

#[tokio::test]
async fn drift_is_detected_and_reapply_restores() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    core.services.cron.create(&a, &input("one")).await.unwrap();
    let good = core.host.cron.content().unwrap();
    core.host
        .cron
        .set_managed(ManagedFile::Present("* * * * * root evil\n".into()));
    assert_eq!(
        core.services.cron.list(&a).await.unwrap().file,
        FileStatus::Drift { missing: false }
    );
    core.services.cron.reapply(&a, false).await.unwrap();
    assert_eq!(core.host.cron.content().unwrap(), good);
    assert_eq!(
        core.services.cron.list(&a).await.unwrap().file,
        FileStatus::InSync
    );
    core.host.cron.set_managed(ManagedFile::Missing);
    assert_eq!(
        core.services.cron.list(&a).await.unwrap().file,
        FileStatus::Drift { missing: true }
    );
}

#[tokio::test]
async fn empty_state_is_in_sync() {
    let core = TestCore::new().await;
    assert_eq!(
        core.services.cron.list(&Actor::cli()).await.unwrap().file,
        FileStatus::InSync
    );
}

#[tokio::test]
async fn unsafe_file_is_refused_until_replaced() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    core.host
        .cron
        .set_managed(ManagedFile::Unsafe("is a symlink".into()));
    assert_eq!(
        kind(core.services.cron.create(&a, &input("x")).await),
        ErrorKind::Conflict
    );
    assert_eq!(entry_count(&core).await, 0);
    assert!(matches!(
        core.services.cron.list(&a).await.unwrap().file,
        FileStatus::Unsafe { .. }
    ));
    core.services.cron.reapply(&a, true).await.unwrap();
    assert!(core.host.cron.content().is_some());
}

#[tokio::test]
async fn toggle_update_delete_regenerate_the_file() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let e = core.services.cron.create(&a, &input("job")).await.unwrap();
    core.services.cron.toggle(&a, &e.id, false).await.unwrap();
    let t = core.host.cron.content().unwrap();
    assert!(t.contains(&format!("# disabled: {}", e.id)) && !t.contains("cron-run crn_"));
    let mut changed = input("job renamed");
    changed.schedule = "@daily".into();
    core.services
        .cron
        .update(&a, &e.id, &changed)
        .await
        .unwrap();
    let t = core.host.cron.content().unwrap();
    assert!(t.contains("# job renamed") && t.contains("@daily root"));
    core.services.cron.delete(&a, &e.id).await.unwrap();
    assert!(!core.host.cron.content().unwrap().contains(&e.id));
    assert_eq!(
        kind(core.services.cron.delete(&a, &e.id).await),
        ErrorKind::NotFound
    );
    assert_eq!(
        kind(core.services.cron.toggle(&a, "crn_none", true).await),
        ErrorKind::NotFound
    );
    for action in ["cron.toggle", "cron.update", "cron.delete"] {
        assert_eq!(audit_count(&core, action, "ok").await, 1, "{action}");
    }
}

#[tokio::test]
async fn preview_and_files() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let p = core
        .services
        .cron
        .preview(&a, "0 0 15 * 1", 5)
        .await
        .unwrap();
    assert_eq!(p.next_runs.len(), 5);
    assert!(p.day_fields_or);
    assert!(matches!(
        kind(core.services.cron.preview(&a, "nope", 5).await),
        ErrorKind::Validation(_)
    ));
    assert!(core.services.cron.files_list(&a).await.unwrap().is_empty());
}

async fn runs(core: &TestCore, id: &str) -> Vec<crate::service::cron::CronRunDto> {
    core.services
        .cron
        .runs_list(&Actor::cli(), id)
        .await
        .unwrap()
}

fn finished(code: Option<i32>, timed_out: bool, out: &str) -> Result<ExecOutcome, CoreError> {
    Ok(ExecOutcome::Finished {
        exit_code: code,
        timed_out,
        output: out.as_bytes().to_vec(),
    })
}

#[tokio::test]
async fn cron_run_records_exit_code_duration_and_output() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let mut inp = input("job");
    inp.timeout_s = Some(30);
    inp.overlap = Overlap::Skip;
    let e = core.services.cron.create(&a, &inp).await.unwrap();

    core.host.cron_exec.push(finished(Some(0), false, "hello"));
    let ok = core.services.cron.run_entry(&a, &e.id).await.unwrap();
    assert_eq!((ok.status.as_str(), ok.exit_code), ("success", Some(0)));
    let spec = core.host.cron_exec.calls().pop().unwrap();
    assert_eq!(spec.command, "echo 100% done");
    assert_eq!(spec.user.name, "deploy");
    assert_eq!(spec.timeout, Some(std::time::Duration::from_secs(30)));
    assert!(spec.skip_if_running);

    core.clock.advance(std::time::Duration::from_secs(5));
    core.host.cron_exec.push(finished(Some(3), false, "boom"));
    core.services.cron.run_entry(&a, &e.id).await.unwrap();
    core.clock.advance(std::time::Duration::from_secs(5));
    core.host.cron_exec.push(finished(None, true, "slow"));
    core.services.cron.run_entry(&a, &e.id).await.unwrap();
    core.clock.advance(std::time::Duration::from_secs(5));
    core.host.cron_exec.push(Ok(ExecOutcome::Skipped));
    core.services.cron.run_entry(&a, &e.id).await.unwrap();

    let r = runs(&core, &e.id).await;
    let statuses: Vec<_> = r.iter().map(|x| x.status.as_str()).collect();
    assert_eq!(statuses, ["skipped", "timeout", "failed", "success"]);
    assert_eq!(r[2].exit_code, Some(3));
    assert_eq!(r[2].output_tail.as_deref(), Some("boom"));
    assert_eq!(r[3].output_tail.as_deref(), Some("hello"));
    assert!(
        r.iter()
            .all(|x| x.finished_at.is_some() && x.duration_ms.is_some())
    );
}

#[tokio::test]
async fn cron_run_failures_that_never_start_are_recorded() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let e = core.services.cron.create(&a, &input("job")).await.unwrap();
    // Spawn failure.
    core.host
        .cron_exec
        .push(Err(CoreError::new(ErrorKind::Internal, "cannot start")));
    assert_eq!(
        core.services
            .cron
            .run_entry(&a, &e.id)
            .await
            .unwrap()
            .status,
        "failed"
    );
    // The Unix user vanished after the entry was saved.
    sqlx::query("UPDATE cron_entries SET run_as_user = 'gone' WHERE id = ?")
        .bind(&e.id)
        .execute(core.db.writer())
        .await
        .unwrap();
    let before = core.host.cron_exec.calls().len();
    core.clock.advance(std::time::Duration::from_secs(1));
    let r = core.services.cron.run_entry(&a, &e.id).await.unwrap();
    assert_eq!(r.status, "failed");
    assert_eq!(
        core.host.cron_exec.calls().len(),
        before,
        "nothing was executed"
    );
    assert!(
        runs(&core, &e.id).await[0]
            .output_tail
            .as_deref()
            .unwrap()
            .contains("unknown user")
    );
    assert!(core.services.cron.list(&a).await.unwrap().entries[0].invalid_user);
    // A non-root name that maps to uid 0 is refused.
    core.host.users.add(UnixUser {
        name: "toor".into(),
        uid: 0,
        gid: 0,
        home: "/root".into(),
        shell: "/bin/sh".into(),
    });
    sqlx::query("UPDATE cron_entries SET run_as_user = 'toor' WHERE id = ?")
        .bind(&e.id)
        .execute(core.db.writer())
        .await
        .unwrap();
    assert_eq!(
        core.services
            .cron
            .run_entry(&a, &e.id)
            .await
            .unwrap()
            .status,
        "failed"
    );
    assert_eq!(core.host.cron_exec.calls().len(), before);
    assert_eq!(
        kind(core.services.cron.run_entry(&a, "crn_none").await),
        ErrorKind::NotFound
    );
}

#[tokio::test]
async fn disabled_entries_do_not_run() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let e = core.services.cron.create(&a, &input("job")).await.unwrap();
    core.services.cron.toggle(&a, &e.id, false).await.unwrap();
    let r = core.services.cron.run_entry(&a, &e.id).await.unwrap();
    assert_eq!(r.status, "skipped");
    assert!(core.host.cron_exec.calls().is_empty());
}

#[tokio::test]
async fn run_history_is_pruned_to_fifty() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let e = core.services.cron.create(&a, &input("job")).await.unwrap();
    for _ in 0..55 {
        core.clock.advance(std::time::Duration::from_secs(60));
        core.services.cron.run_entry(&a, &e.id).await.unwrap();
    }
    assert_eq!(runs(&core, &e.id).await.len(), 50);
    // Runs older than 30 days go too, even when fewer than 50 remain.
    core.clock
        .advance(std::time::Duration::from_secs(31 * 24 * 3600));
    core.services.cron.run_entry(&a, &e.id).await.unwrap();
    assert_eq!(runs(&core, &e.id).await.len(), 1);
}

#[tokio::test]
async fn output_tail_is_capped() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let e = core.services.cron.create(&a, &input("job")).await.unwrap();
    let big = "x".repeat(crate::cron::OUTPUT_TAIL_BYTES + 100);
    core.host.cron_exec.push(finished(Some(0), false, &big));
    core.services.cron.run_entry(&a, &e.id).await.unwrap();
    assert_eq!(
        runs(&core, &e.id).await[0]
            .output_tail
            .as_ref()
            .unwrap()
            .len(),
        crate::cron::OUTPUT_TAIL_BYTES
    );
}

#[tokio::test]
async fn patch_changes_only_the_given_fields_and_checks_permission_first() {
    use crate::service::cron::CronPatch;
    let core = TestCore::new().await;
    let a = Actor::cli();
    let e = core.services.cron.create(&a, &input("job")).await.unwrap();
    let p = CronPatch {
        schedule: Some("@hourly".into()),
        overlap: Some(Overlap::Skip),
        ..Default::default()
    };
    let out = core.services.cron.patch(&a, &e.id, &p).await.unwrap();
    assert_eq!(
        (out.schedule.as_str(), out.overlap.as_str()),
        ("@hourly", "skip")
    );
    assert_eq!(
        (out.command.as_str(), out.run_as_user.as_str()),
        ("echo 100% done", "deploy")
    );
    assert_eq!(
        kind(core.services.cron.patch(&a, "crn_none", &p).await),
        ErrorKind::NotFound
    );
    let bad = CronPatch {
        schedule: Some("nope".into()),
        ..Default::default()
    };
    assert!(matches!(
        kind(core.services.cron.patch(&a, &e.id, &bad).await),
        ErrorKind::Validation(_)
    ));
}
