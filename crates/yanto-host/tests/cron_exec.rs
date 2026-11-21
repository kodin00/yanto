//! Host-integration tests for running a Cron Entry's command as a user (the current one here).

use std::time::{Duration, Instant};

use nix::unistd::{getgid, getuid};
use yanto_core::ErrorKind;
use yanto_core::cron::OUTPUT_TAIL_BYTES;
use yanto_core::ports::{CronExec, ExecOutcome, ExecSpec, UnixUser};
use yanto_host::HostCronExec;

fn user(home: &std::path::Path) -> UnixUser {
    UnixUser {
        name: "tester".into(),
        uid: getuid().as_raw(),
        gid: getgid().as_raw(),
        home: home.to_path_buf(),
        shell: "/bin/sh".into(),
    }
}

fn spec(home: &std::path::Path, cmd: &str) -> ExecSpec {
    ExecSpec {
        entry_id: "crn_t".into(),
        command: cmd.into(),
        user: user(home),
        timeout: None,
        skip_if_running: false,
    }
}

fn exec(run: &std::path::Path) -> HostCronExec {
    HostCronExec::new(run).with_kill_grace(Duration::from_millis(200))
}

fn finished(o: ExecOutcome) -> (Option<i32>, bool, String) {
    match o {
        ExecOutcome::Finished {
            exit_code,
            timed_out,
            output,
        } => (
            exit_code,
            timed_out,
            String::from_utf8_lossy(&output).into_owned(),
        ),
        ExecOutcome::Skipped => panic!("unexpected skip"),
    }
}

#[test]
fn records_exit_code_and_combined_output_and_percent_signs() {
    let d = tempfile::tempdir().unwrap();
    let e = exec(&d.path().join("run"));
    let (code, to, out) = finished(
        e.run(&spec(
            d.path(),
            "echo out; echo err >&2; date +%Y >/dev/null; echo 100%; exit 3",
        ))
        .unwrap(),
    );
    assert_eq!((code, to), (Some(3), false));
    assert!(
        out.contains("out") && out.contains("err") && out.contains("100%"),
        "{out}"
    );
    let (code, _, _) = finished(e.run(&spec(d.path(), "true")).unwrap());
    assert_eq!(code, Some(0));
}

#[test]
fn environment_and_working_directory_follow_cron() {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().canonicalize().unwrap();
    let (_, _, out) = finished(
        exec(&d.path().join("run"))
            .run(&spec(&home, "echo $HOME $USER $LOGNAME $SHELL; echo $PATH; pwd; env | wc -l; echo \"${SECRET_FROM_PARENT:-unset}\""))
            .unwrap(),
    );
    let lines: Vec<_> = out.lines().collect();
    assert_eq!(
        lines[0],
        format!("{} tester tester /bin/sh", home.display())
    );
    assert_eq!(
        lines[1],
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    );
    assert_eq!(lines[2], home.display().to_string());
    assert_eq!(*lines.last().unwrap(), "unset");
}

#[test]
fn timeout_sends_term_then_kills_the_whole_process_group() {
    let d = tempfile::tempdir().unwrap();
    let e = exec(&d.path().join("run"));
    let mut s = spec(d.path(), "sleep 30");
    s.timeout = Some(Duration::from_millis(200));
    let t = Instant::now();
    let (code, timed_out, _) = finished(e.run(&s).unwrap());
    assert!(timed_out && code.is_none());
    assert!(t.elapsed() < Duration::from_secs(5));

    // A shell that ignores SIGTERM and has a child is still killed after the grace period.
    let mut s = spec(d.path(), "trap '' TERM; sleep 30 & sleep 30");
    s.timeout = Some(Duration::from_millis(200));
    let t = Instant::now();
    let (_, timed_out, _) = finished(e.run(&s).unwrap());
    assert!(timed_out);
    assert!(
        t.elapsed() < Duration::from_secs(8),
        "took {:?}",
        t.elapsed()
    );
}

#[test]
fn overlap_skip_refuses_a_second_concurrent_run() {
    let d = tempfile::tempdir().unwrap();
    let run = d.path().join("run");
    let mut slow = spec(d.path(), "sleep 1");
    slow.skip_if_running = true;
    let (e1, s1) = (exec(&run), slow.clone());
    let h = std::thread::spawn(move || e1.run(&s1).unwrap());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(exec(&run).run(&slow).unwrap(), ExecOutcome::Skipped);
    assert!(matches!(
        h.join().unwrap(),
        ExecOutcome::Finished {
            exit_code: Some(0),
            ..
        }
    ));
    // Once finished, the next run goes ahead.
    let mut quick = spec(d.path(), "true");
    quick.skip_if_running = true;
    assert!(matches!(
        exec(&run).run(&quick).unwrap(),
        ExecOutcome::Finished { .. }
    ));
    // Entries that allow overlap never take the lock.
    let (e2, s2) = (exec(&run), spec(d.path(), "sleep 1"));
    let h = std::thread::spawn(move || e2.run(&s2).unwrap());
    std::thread::sleep(Duration::from_millis(300));
    assert!(matches!(
        exec(&run).run(&spec(d.path(), "true")).unwrap(),
        ExecOutcome::Finished { .. }
    ));
    h.join().unwrap();
}

#[test]
fn output_is_capped_to_the_last_64_kib() {
    let d = tempfile::tempdir().unwrap();
    let cmd = "head -c 300000 /dev/zero | tr '\\0' a; printf END";
    let (_, _, out) = finished(
        exec(&d.path().join("run"))
            .run(&spec(d.path(), cmd))
            .unwrap(),
    );
    assert_eq!(out.len(), OUTPUT_TAIL_BYTES);
    assert!(out.ends_with("aEND"));
}

#[test]
fn switching_to_another_user_needs_root() {
    if nix::unistd::geteuid().is_root() {
        return; // root can switch; covered by the CLI test when run as root
    }
    let d = tempfile::tempdir().unwrap();
    let mut u = spec(d.path(), "true");
    u.user.uid += 1;
    let e = exec(&d.path().join("run")).run(&u).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unavailable);
}
