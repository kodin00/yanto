//! `yanto self-update` (docs/rewrite/05 section 7, D31, OQ-10, OQ-15).

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Args;
use semver::Version;
use yanto_core::Config;

use crate::ops::host::{Host, RealHost};
use crate::ops::install::{run_systemctl, wait_healthy};
use crate::ops::layout::{BIN_PATH, Layout, UNIT_NAME};
use crate::ops::prompt::{InquirePrompter, Prompter};
use crate::ops::release::{self, DEFAULT_REPO};
use crate::ops::update;

#[derive(Debug, Clone, Args, Default)]
pub struct SelfUpdateArgs {
    /// Report the available version only
    #[arg(long)]
    pub check: bool,
    /// Install this tag (default: latest non-prerelease)
    #[arg(long, value_name = "vX.Y.Z")]
    pub version: Option<String>,
    /// Consider pre-releases when picking the newest version
    #[arg(long)]
    pub allow_prerelease: bool,
    /// Do not ask for confirmation
    #[arg(long)]
    pub yes: bool,
    /// Replace the binary but do not restart the service
    #[arg(long)]
    pub no_restart: bool,
    /// Restore the previous binary (yanto.prev)
    #[arg(long)]
    pub rollback: bool,
    /// Allow a non-standard install path or a downgrade
    #[arg(long)]
    pub force: bool,
    /// Operate under this directory prefix instead of /
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
}

pub fn run(config_path: &Path, args: &SelfUpdateArgs) -> anyhow::Result<()> {
    let layout = Layout::from_root(args.root.as_deref());
    let host = RealHost;
    let cfg = Config::load(&layout.path(config_path)).ok();

    if args.rollback {
        return rollback(&host, &layout, cfg.as_ref(), args.no_restart);
    }

    if layout.is_system() && !args.force {
        let exe = std::env::current_exe().context("locating the running binary")?;
        if exe != Path::new(BIN_PATH) {
            bail!(
                "running binary is {} but self-update manages {BIN_PATH}; pass --force to proceed",
                exe.display()
            );
        }
    }

    let repo = std::env::var("YANTO_REPO").unwrap_or_else(|_| DEFAULT_REPO.into());
    let token = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty());
    let how = release::choose_resolver(
        args.version.as_deref(),
        args.allow_prerelease,
        token.is_some(),
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let tag = rt.block_on(release::resolve_tag(&repo, &how, token.as_deref()))?;
    let target = release::parse_tag(&tag)?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    match decide(&current, &target, args.version.is_some(), args.force) {
        Decision::UpToDate => {
            println!("yanto {current} is up to date (latest: {tag})");
            return Ok(());
        }
        Decision::RefuseDowngrade => bail!(
            "{tag} is older than the installed {current}; downgrades can break forward-only migrations. Pass --force to override"
        ),
        Decision::Update => {}
    }
    println!("update available: {current} -> {tag}");
    if args.check {
        return Ok(());
    }

    if !args.yes {
        let mut ip = InquirePrompter;
        if !ip.confirm(&format!("Update yanto to {tag}?"), true)? {
            bail!("aborted");
        }
    }

    let asset = release::asset_name();
    let bytes = rt.block_on(release::download(&release::asset_url(&repo, &tag, asset)))?;
    let sums = String::from_utf8(rt.block_on(release::download(&release::asset_url(
        &repo,
        &tag,
        "SHA256SUMS",
    )))?)
    .context("SHA256SUMS is not UTF-8")?;
    release::verify_checksum(&sums, asset, &bytes)?;
    println!("SHA256 verified for {asset}");

    let data_dir = cfg.as_ref().map_or_else(
        || Config::default().paths.data_dir,
        |c| c.paths.data_dir.clone(),
    );
    let staged = layout
        .path(&data_dir)
        .join("tmp")
        .join(format!("yanto-update-{tag}"));
    stage(&bytes, &staged)?;
    let outcome = (|| {
        attest(&host, &staged, &repo)?;
        verify_runs(&host, &staged, &target)
    })();
    let _ = std::fs::remove_file(&staged);
    outcome?;

    let out = perform(&host, &layout, cfg.as_ref(), &bytes, !args.no_restart, 30)?;
    if let Some(db) = out.db_backup {
        println!("pre-update DB copy: {}", db.display());
    }
    println!(
        "updated to {tag}{}",
        if args.no_restart {
            " (not restarted)"
        } else {
            ""
        }
    );
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    UpToDate,
    RefuseDowngrade,
    Update,
}

/// `explicit` = the operator named a version, so an equal version reinstalls.
pub fn decide(current: &Version, target: &Version, explicit: bool, force: bool) -> Decision {
    use std::cmp::Ordering::*;
    match target.cmp(current) {
        Greater => Decision::Update,
        Equal if explicit => Decision::Update,
        Equal => Decision::UpToDate,
        Less if force => Decision::Update,
        Less if explicit => Decision::RefuseDowngrade,
        Less => Decision::UpToDate,
    }
}

fn stage(bytes: &[u8], path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().context("no parent")?)?;
    std::fs::write(path, bytes)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// `gh attestation verify` when gh is installed (D27); otherwise integrity only.
fn attest(host: &dyn Host, file: &Path, repo: &str) -> anyhow::Result<()> {
    if host.which("gh").is_none() {
        println!(
            "gh not found: skipping build-provenance attestation check (SHA256 proves integrity only)"
        );
        return Ok(());
    }
    let f = file.to_string_lossy();
    let out = host.run("gh", &["attestation", "verify", &f, "--repo", repo])?;
    if !out.success {
        bail!("gh attestation verify failed: {}", out.stderr.trim());
    }
    println!("build provenance attestation verified");
    Ok(())
}

fn verify_runs(host: &dyn Host, file: &Path, want: &Version) -> anyhow::Result<()> {
    let out = host
        .run(&file.to_string_lossy(), &["version"])
        .context("the downloaded binary could not be executed")?;
    let expected = format!("yanto {want}");
    if !out.success || out.stdout.trim() != expected {
        bail!(
            "downloaded binary reported {:?}, expected {expected:?}",
            out.stdout.trim()
        );
    }
    Ok(())
}

#[derive(Debug)]
pub struct Outcome {
    pub db_backup: Option<PathBuf>,
}

/// Back up the DB, swap the binary, restart and health-check; roll the binary
/// back if the service does not come up healthy.
pub fn perform(
    host: &dyn Host,
    layout: &Layout,
    cfg: Option<&Config>,
    new_bin: &[u8],
    restart: bool,
    attempts: u32,
) -> anyhow::Result<Outcome> {
    let data_dir = cfg.map_or_else(
        || Config::default().paths.data_dir,
        |c| c.paths.data_dir.clone(),
    );
    let db_backup = update::backup_db(layout, &data_dir)?;
    update::swap_in(layout, new_bin)?;
    if restart && layout.is_system() {
        let healthy = restart_and_wait(host, cfg, attempts);
        if let Err(e) = healthy {
            update::restore_prev(layout)?;
            let _ = run_systemctl(host, &["restart", UNIT_NAME]);
            bail!(
                "{e:#}; the previous binary was restored and the service restarted. The pre-update DB copy is kept{}. If a migration already ran, restore it manually (stop yanto, copy the backup over yanto.db)",
                db_backup
                    .as_ref()
                    .map_or(String::new(), |p| format!(" at {}", p.display()))
            );
        }
    }
    Ok(Outcome { db_backup })
}

fn restart_and_wait(host: &dyn Host, cfg: Option<&Config>, attempts: u32) -> anyhow::Result<()> {
    run_systemctl(host, &["restart", UNIT_NAME])?;
    let Some(c) = cfg else { return Ok(()) };
    let bind = c.server.bind.parse().context("server.bind")?;
    if !wait_healthy(host, bind, c.server.port, attempts) {
        bail!("service did not become healthy after the update");
    }
    Ok(())
}

fn rollback(
    host: &dyn Host,
    layout: &Layout,
    cfg: Option<&Config>,
    no_restart: bool,
) -> anyhow::Result<()> {
    update::restore_prev(layout)?;
    println!("restored {}", update::prev(layout).display());
    if !no_restart && layout.is_system() {
        run_systemctl(host, &["restart", UNIT_NAME])?;
    }
    let data_dir = cfg.map_or_else(
        || Config::default().paths.data_dir,
        |c| c.paths.data_dir.clone(),
    );
    let dir = layout.path(&data_dir).join("backups/yanto");
    let mut backups: Vec<_> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("pre-update-"))
        })
        .collect();
    backups.sort();
    println!(
        "Migrations are forward-only. If the newer version already migrated the database, restore the pre-update copy manually:"
    );
    println!("  1. systemctl stop yanto");
    match backups.last() {
        Some(b) => println!(
            "  2. cp {} {}/yanto.db   (remove yanto.db-wal and yanto.db-shm first)",
            b.display(),
            data_dir.display()
        ),
        None => println!("  2. no pre-update copy found under {}", dir.display()),
    }
    println!("  3. systemctl start yanto");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::host::{CmdOutput, FakeHost};
    use std::fs;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn decision_matrix() {
        assert_eq!(
            decide(&v("1.0.0"), &v("1.1.0"), false, false),
            Decision::Update
        );
        assert_eq!(
            decide(&v("1.0.0"), &v("1.0.0"), false, false),
            Decision::UpToDate
        );
        assert_eq!(
            decide(&v("1.0.0"), &v("1.0.0"), true, false),
            Decision::Update
        );
        assert_eq!(
            decide(&v("1.1.0"), &v("1.0.0"), true, false),
            Decision::RefuseDowngrade
        );
        assert_eq!(
            decide(&v("1.1.0"), &v("1.0.0"), true, true),
            Decision::Update
        );
        assert_eq!(
            decide(&v("1.1.0"), &v("1.0.0"), false, false),
            Decision::UpToDate
        );
    }

    fn system_like() -> (tempfile::TempDir, Layout) {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path());
        fs::create_dir_all(update::bin(&l).parent().unwrap()).unwrap();
        fs::write(update::bin(&l), b"old").unwrap();
        (d, l)
    }

    #[test]
    fn perform_without_restart_swaps_and_backs_up_db() {
        let (_d, l) = system_like();
        crate::ops::install::migrate(&l.path("/var/lib/yanto/yanto.db")).unwrap();
        let out = perform(&FakeHost::default(), &l, None, b"new", false, 1).unwrap();
        assert_eq!(fs::read(update::bin(&l)).unwrap(), b"new");
        assert_eq!(fs::read(update::prev(&l)).unwrap(), b"old");
        assert!(out.db_backup.unwrap().exists());
    }

    #[test]
    fn verify_runs_checks_reported_version() {
        let mut h = FakeHost::default();
        h.respond("/tmp/x version", CmdOutput::ok("yanto 1.2.3\n"));
        verify_runs(&h, Path::new("/tmp/x"), &v("1.2.3")).unwrap();
        assert!(verify_runs(&h, Path::new("/tmp/x"), &v("1.2.4")).is_err());
        assert!(verify_runs(&FakeHost::default(), Path::new("/tmp/x"), &v("1.2.3")).is_err());
    }

    #[test]
    fn attestation_skipped_without_gh_and_enforced_with_it() {
        let mut h = FakeHost::default();
        attest(&h, Path::new("/tmp/x"), "o/r").unwrap();
        h.binaries.insert("gh".into());
        h.respond("gh attestation verify /tmp/x --repo o/r", CmdOutput::fail());
        assert!(attest(&h, Path::new("/tmp/x"), "o/r").is_err());
        h.respond("gh attestation verify /tmp/x --repo o/r", CmdOutput::ok(""));
        attest(&h, Path::new("/tmp/x"), "o/r").unwrap();
    }
}
