//! Host-integration tests for the cron file writer against fixture `/etc` trees.

use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use nix::unistd::{getgid, getuid};
use yanto_core::ports::{CronContent, CronPort, CronSourceKind, ManagedFile};
use yanto_core::{CoreError, ErrorKind};
use yanto_host::HostCron;

fn me() -> (u32, u32) {
    (getuid().as_raw(), getgid().as_raw())
}

struct Fx {
    _dir: tempfile::TempDir,
    etc: PathBuf,
    spool: PathBuf,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let etc = dir.path().join("etc");
    let spool = dir.path().join("spool");
    std::fs::create_dir_all(etc.join("cron.d")).unwrap();
    std::fs::create_dir_all(spool.join("crontabs")).unwrap();
    std::fs::write(
        etc.join("crontab"),
        "# system\n17 * * * * root run-parts /etc/cron.hourly\n",
    )
    .unwrap();
    std::fs::write(etc.join("cron.d/logrotate"), "0 1 * * * root logrotate\n").unwrap();
    std::fs::write(etc.join("cron.d/.placeholder"), "").unwrap();
    std::fs::create_dir_all(etc.join("cron.daily")).unwrap();
    std::fs::write(etc.join("cron.daily/apt-compat"), "#!/bin/sh\n").unwrap();
    std::fs::write(spool.join("crontabs/alice"), "* * * * * echo hi\n").unwrap();
    Fx {
        _dir: dir,
        etc,
        spool,
    }
}

fn cron(fx: &Fx) -> HostCron {
    HostCron::new(&fx.etc, &fx.spool, me())
        .with_binary("/usr/local/bin/yanto")
        .without_service_check()
}

/// (relative path, bytes, mode) of every file under `root`, for before/after comparison.
fn tree(root: &Path) -> Vec<(String, Vec<u8>, u32)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>, u32)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                walk(&p, root, out);
            } else if m.is_file() {
                out.push((
                    p.strip_prefix(root).unwrap().display().to_string(),
                    std::fs::read(&p).unwrap(),
                    m.mode() & 0o7777,
                ));
            }
        }
    }
    let mut v = Vec::new();
    walk(root, root, &mut v);
    v
}

const CONTENT: &str = "# Managed by Yanto\nSHELL=/bin/sh\n\n# job\n* * * * * root /usr/local/bin/yanto cron-run crn_1\n";

#[test]
fn writes_a_0644_file_atomically_and_leaves_no_temp() {
    let fx = fixture();
    let c = cron(&fx);
    assert_eq!(c.read_managed().unwrap(), ManagedFile::Missing);
    c.write_managed(CONTENT, false).unwrap();
    let p = fx.etc.join("cron.d/yanto");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), CONTENT);
    let m = std::fs::metadata(&p).unwrap();
    assert_eq!(m.mode() & 0o7777, 0o644);
    assert_eq!(m.uid(), me().0);
    assert!(
        !fx.etc.join(".yanto-cron.tmp").exists(),
        "temp file is gone"
    );
    assert_eq!(
        c.read_managed().unwrap(),
        ManagedFile::Present(CONTENT.into())
    );
    // The temp name is dotted, so cron (which skips dotted names) could never read it.
    assert!(".yanto-cron.tmp".contains('.'));
    // Rewriting replaces the content.
    c.write_managed("X=1\n", false).unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "X=1\n");
}

#[test]
fn foreign_cron_files_are_never_modified() {
    let fx = fixture();
    let before = tree(&fx.etc)
        .into_iter()
        .chain(tree(&fx.spool))
        .collect::<Vec<_>>();
    let c = cron(&fx);
    c.write_managed(CONTENT, false).unwrap();
    c.write_managed("A=1\n", false).unwrap();
    c.system_sources().unwrap();
    let mut after: Vec<_> = tree(&fx.etc).into_iter().chain(tree(&fx.spool)).collect();
    after.retain(|(p, _, _)| p != "cron.d/yanto");
    assert_eq!(before, after, "only cron.d/yanto may differ");
}

#[test]
fn refuses_to_write_through_a_symlink_unless_replacing() {
    let fx = fixture();
    let target = fx.etc.join("elsewhere");
    std::fs::write(&target, "precious\n").unwrap();
    symlink(&target, fx.etc.join("cron.d/yanto")).unwrap();
    let c = cron(&fx);
    assert!(matches!(c.read_managed().unwrap(), ManagedFile::Unsafe(_)));
    let e = c.write_managed(CONTENT, false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious\n");
    // The Owner may replace it: the link is swapped for a regular file, the target is untouched.
    c.write_managed(CONTENT, true).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious\n");
    assert!(
        !std::fs::symlink_metadata(fx.etc.join("cron.d/yanto"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn wrong_owner_or_writable_mode_is_unsafe() {
    let fx = fixture();
    let c = cron(&fx);
    c.write_managed(CONTENT, false).unwrap();
    let p = fx.etc.join("cron.d/yanto");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o664)).unwrap();
    assert!(matches!(c.read_managed().unwrap(), ManagedFile::Unsafe(m) if m.contains("writable")));
    assert!(c.write_managed(CONTENT, false).is_err());
    c.write_managed(CONTENT, true).unwrap();
    assert_eq!(std::fs::metadata(&p).unwrap().mode() & 0o7777, 0o644);
    // Expecting a different owner than the file's real one.
    let other = HostCron::new(&fx.etc, &fx.spool, (me().0 + 1, me().1)).without_service_check();
    assert!(
        matches!(other.read_managed().unwrap(), ManagedFile::Unsafe(m) if m.contains("owned by"))
    );
}

#[test]
fn missing_cron_dir_and_missing_newline_are_errors() {
    let fx = fixture();
    let c = cron(&fx);
    assert!(c.write_managed("no newline", false).is_err());
    std::fs::remove_dir_all(fx.etc.join("cron.d")).unwrap();
    let e: CoreError = c.write_managed(CONTENT, false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unavailable);
}

#[test]
fn read_only_view_lists_other_sources_but_not_yanto_itself() {
    let fx = fixture();
    let c = cron(&fx);
    c.write_managed(CONTENT, false).unwrap();
    // A big file, a binary file and a symlink in the spool.
    std::fs::write(fx.etc.join("cron.d/big"), "x".repeat(300 * 1024)).unwrap();
    std::fs::write(fx.etc.join("cron.d/bin"), [0xff, 0xfe, 0x00, 0x01]).unwrap();
    symlink("/etc/passwd", fx.etc.join("cron.d/link")).unwrap();
    let s = c.system_sources().unwrap();
    let find = |suffix: &str| {
        s.iter()
            .find(|x| x.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("{suffix}"))
    };
    assert_eq!(find("etc/crontab").kind, CronSourceKind::Crontab);
    assert!(
        matches!(&find("cron.d/logrotate").content, CronContent::Text(t) if t.contains("logrotate"))
    );
    assert!(
        matches!(&find("cron.d/big").content, CronContent::Truncated(t) if t.len() == 256 * 1024)
    );
    assert!(matches!(
        find("cron.d/bin").content,
        CronContent::Unreadable(_)
    ));
    assert!(matches!(
        find("cron.d/link").content,
        CronContent::Unreadable(_)
    ));
    assert_eq!(find("cron.daily/apt-compat").kind, CronSourceKind::Periodic);
    assert_eq!(find("crontabs/alice").kind, CronSourceKind::UserSpool);
    assert!(
        s.iter()
            .all(|x| !x.path.ends_with("cron.d/yanto") && !x.path.ends_with(".placeholder"))
    );
}

#[test]
fn non_debian_spool_layout_is_supported() {
    let fx = fixture();
    std::fs::remove_dir_all(fx.spool.join("crontabs")).unwrap();
    std::fs::write(fx.spool.join("bob"), "@daily x\n").unwrap();
    let s = cron(&fx).system_sources().unwrap();
    assert!(
        s.iter()
            .any(|x| x.path.ends_with("spool/bob") && x.kind == CronSourceKind::UserSpool)
    );
}

#[test]
fn binary_path_defaults_to_the_running_executable() {
    let fx = fixture();
    let c = HostCron::new(&fx.etc, &fx.spool, me());
    assert!(Path::new(&c.yanto_binary().unwrap()).is_absolute());
    assert_eq!(cron(&fx).yanto_binary().unwrap(), "/usr/local/bin/yanto");
}
