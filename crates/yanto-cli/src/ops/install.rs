//! Applying an install: data dir tree, app key, config, migrations, unit,
//! service start (05 section 3.4). Every step is idempotent.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, bail};

use super::host::Host;
use super::layout::{CRON_PATH, Layout, UNIT_NAME, UNIT_PATH};
use super::params::InstallParams;
use super::render::{render_config, render_unit, ssh_dir_for};
use super::setup_code;
use super::wizard::home_of;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Fresh,
    /// Rewrite config + unit from new values; keep data, DB and `app.key`.
    Reconfigure,
    /// Keep the config; re-render the unit and restart.
    UpgradeOnly,
}

#[derive(Debug, Default)]
pub struct ApplyReport {
    pub steps: Vec<String>,
    /// Plaintext setup code, to be printed exactly once by the caller.
    pub setup_code: Option<String>,
    /// `None` when the service was not started.
    pub healthy: Option<bool>,
}

/// An install exists when the config file or the unit is present.
pub fn existing_install(layout: &Layout, config_path: &Path) -> bool {
    layout.path(config_path).exists() || layout.path(UNIT_PATH).exists()
}

/// Write `bytes` to `path` via temp file + rename with the given mode.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> anyhow::Result<()> {
    let dir = path.parent().context("path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    // `mode` above is subject to umask; make it exact.
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

fn mkdir_private(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Create the data dir tree (01 section 8.2) and `keys/app.key` if absent.
/// Returns true when a new key was generated.
pub fn ensure_data_layout(data_dir: &Path) -> anyhow::Result<bool> {
    for sub in [
        "",
        "keys",
        "ssh",
        "projects",
        "backups",
        "backups/projects",
        "backups/yanto",
        "registry",
        "tmp",
    ] {
        mkdir_private(&data_dir.join(sub))?;
    }
    let key = data_dir.join("keys/app.key");
    if key.exists() {
        return Ok(false);
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no secure randomness: {e}"))?;
    write_atomic(&key, &bytes, 0o600)?;
    Ok(true)
}

pub fn run_systemctl(host: &dyn Host, args: &[&str]) -> anyhow::Result<()> {
    let out = host
        .run("systemctl", args)
        .with_context(|| format!("running systemctl {}", args.join(" ")))?;
    if !out.success {
        bail!("systemctl {} failed: {}", args.join(" "), out.stderr.trim());
    }
    Ok(())
}

pub fn migrate(db_path: &Path) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let db = yanto_core::Db::open(db_path)
            .await
            .context("opening database / running migrations")?;
        db.close().await;
        anyhow::Ok(())
    })
}

pub struct ApplyOptions {
    /// Run systemctl (false under a non-system root).
    pub systemctl: bool,
    pub health_attempts: u32,
}

pub fn apply(
    host: &dyn Host,
    layout: &Layout,
    p: &InstallParams,
    mode: Mode,
    opts: &ApplyOptions,
) -> anyhow::Result<ApplyReport> {
    let mut r = ApplyReport::default();
    let data_dir = layout.path(&p.data_dir);

    // 1. data dir + app key
    if ensure_data_layout(&data_dir)? {
        r.steps.push(format!(
            "generated {}/keys/app.key (0600)",
            p.data_dir.display()
        ));
    }
    r.steps
        .push(format!("data dir ready at {}", p.data_dir.display()));

    // 2. config
    if mode != Mode::UpgradeOnly {
        write_atomic(
            &layout.path(&p.config_path),
            render_config(p).as_bytes(),
            0o600,
        )?;
        r.steps
            .push(format!("wrote {} (0600)", p.config_path.display()));
    }

    // 3. migrations
    let db_path = data_dir.join("yanto.db");
    migrate(&db_path)?;
    r.steps.push("database migrations applied".into());

    // 4. setup code (fresh installs without an Owner only)
    if mode == Mode::Fresh && !setup_code::owner_exists(&db_path)? {
        r.setup_code = setup_code::issue(&db_path)?;
    }

    // 5. unit
    let home = home_of(layout, &p.ssh_user);
    let unit = render_unit(p, ssh_dir_for(home.as_deref()).as_deref());
    write_atomic(&layout.path(UNIT_PATH), unit.as_bytes(), 0o644)?;
    r.steps.push(format!("wrote {UNIT_PATH}"));
    if opts.systemctl {
        run_systemctl(host, &["daemon-reload"])?;
        r.steps.push("systemctl daemon-reload".into());
    }

    // 6. start
    if p.start && opts.systemctl {
        run_systemctl(host, &["enable", UNIT_NAME])?;
        run_systemctl(host, &["restart", UNIT_NAME])?;
        r.steps.push("enabled and (re)started yanto.service".into());
        let ok = wait_healthy(host, p.bind, p.port, opts.health_attempts);
        r.healthy = Some(ok);
        if !ok {
            bail!(
                "yanto did not become healthy on {}:{} within the timeout; config and unit were left in place. Inspect: journalctl -u yanto -n 50",
                p.bind,
                p.port
            );
        }
        r.steps
            .push(format!("healthy on http://{}:{}/healthz", p.bind, p.port));
    }
    Ok(r)
}

pub fn wait_healthy(host: &dyn Host, bind: std::net::IpAddr, port: u16, attempts: u32) -> bool {
    for i in 0..attempts {
        if host.healthy(bind, port) {
            return true;
        }
        if i + 1 < attempts {
            host.sleep(Duration::from_secs(1));
        }
    }
    false
}

/// `/etc/cron.d/yanto` under the layout (for uninstall).
pub fn cron_file(layout: &Layout) -> std::path::PathBuf {
    layout.path(CRON_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::host::{CmdOutput, FakeHost};
    use crate::ops::render::fixtures::params;
    use yanto_core::Config;

    fn temp() -> (tempfile::TempDir, Layout, InstallParams) {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path());
        let mut p = params();
        p.start = false;
        fs::create_dir_all(l.path("/etc")).unwrap();
        fs::write(
            l.path("/etc/passwd"),
            "deploy:x:1000:1000::/home/deploy:/bin/sh\n",
        )
        .unwrap();
        (d, l, p)
    }

    fn opts() -> ApplyOptions {
        ApplyOptions {
            systemctl: false,
            health_attempts: 3,
        }
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn script_systemctl(h: &mut FakeHost) {
        for c in [
            "systemctl daemon-reload",
            "systemctl enable yanto",
            "systemctl restart yanto",
        ] {
            h.respond(c, CmdOutput::ok(""));
        }
    }

    #[test]
    fn install_into_temp_root_writes_everything() {
        let (_d, l, p) = temp();
        let r = apply(&FakeHost::default(), &l, &p, Mode::Fresh, &opts()).unwrap();
        assert!(r.healthy.is_none());

        let data = l.path("/var/lib/yanto");
        for sub in [
            "keys",
            "ssh",
            "projects",
            "backups/projects",
            "backups/yanto",
            "registry",
            "tmp",
        ] {
            assert_eq!(mode_of(&data.join(sub)), 0o700, "{sub}");
        }
        assert_eq!(mode_of(&data), 0o700);
        assert_eq!(fs::read(data.join("keys/app.key")).unwrap().len(), 32);
        assert_eq!(mode_of(&data.join("keys/app.key")), 0o600);
        assert!(data.join("yanto.db").exists());

        let cfg = l.path("/etc/yanto/config.toml");
        assert_eq!(mode_of(&cfg), 0o600);
        assert_eq!(Config::load(&cfg).unwrap().server.port, 9197);

        let unit = fs::read_to_string(l.path(UNIT_PATH)).unwrap();
        assert!(
            unit.contains("ExecStart=/usr/local/bin/yanto serve --config /etc/yanto/config.toml")
        );
        assert!(unit.contains("/home/deploy/.ssh"));
        assert!(existing_install(&l, Path::new("/etc/yanto/config.toml")));
    }

    #[test]
    fn rerun_keeps_app_key_and_is_idempotent() {
        let (_d, l, p) = temp();
        let h = FakeHost::default();
        apply(&h, &l, &p, Mode::Fresh, &opts()).unwrap();
        let key = l.path("/var/lib/yanto/keys/app.key");
        let before = fs::read(&key).unwrap();
        let mut p2 = p.clone();
        p2.port = 9300;
        let r = apply(&h, &l, &p2, Mode::Reconfigure, &opts()).unwrap();
        assert_eq!(
            fs::read(&key).unwrap(),
            before,
            "app.key must never be regenerated"
        );
        assert!(!r.steps.iter().any(|s| s.contains("generated")));
        assert!(r.setup_code.is_none());
        assert_eq!(
            Config::load(&l.path("/etc/yanto/config.toml"))
                .unwrap()
                .server
                .port,
            9300
        );
    }

    #[test]
    fn upgrade_only_keeps_config() {
        let (_d, l, p) = temp();
        let h = FakeHost::default();
        apply(&h, &l, &p, Mode::Fresh, &opts()).unwrap();
        let mut p2 = p.clone();
        p2.port = 1234;
        apply(&h, &l, &p2, Mode::UpgradeOnly, &opts()).unwrap();
        assert_eq!(
            Config::load(&l.path("/etc/yanto/config.toml"))
                .unwrap()
                .server
                .port,
            9197
        );
    }

    #[test]
    fn systemd_flow_orders_commands_and_checks_health() {
        let (_d, l, mut p) = temp();
        p.start = true;
        let mut h = FakeHost::default();
        script_systemctl(&mut h);
        let o = ApplyOptions {
            systemctl: true,
            health_attempts: 2,
        };
        let r = apply(&h, &l, &p, Mode::Fresh, &o).unwrap();
        assert_eq!(r.healthy, Some(true));
        assert_eq!(
            *h.calls.borrow(),
            vec![
                "systemctl daemon-reload",
                "systemctl enable yanto",
                "systemctl restart yanto"
            ]
        );
    }

    #[test]
    fn no_start_only_reloads() {
        let (_d, l, p) = temp();
        let mut h = FakeHost::default();
        script_systemctl(&mut h);
        let o = ApplyOptions {
            systemctl: true,
            health_attempts: 2,
        };
        apply(&h, &l, &p, Mode::Fresh, &o).unwrap();
        assert_eq!(*h.calls.borrow(), vec!["systemctl daemon-reload"]);
    }

    #[test]
    fn unhealthy_service_fails_but_leaves_files() {
        let (_d, l, mut p) = temp();
        p.start = true;
        let mut h = FakeHost {
            healthy: false,
            ..FakeHost::default()
        };
        script_systemctl(&mut h);
        let o = ApplyOptions {
            systemctl: true,
            health_attempts: 2,
        };
        let e = apply(&h, &l, &p, Mode::Fresh, &o).unwrap_err();
        assert!(e.to_string().contains("journalctl -u yanto"));
        assert!(l.path("/etc/yanto/config.toml").exists());
    }

    #[test]
    fn systemctl_failure_surfaces() {
        let (_d, l, p) = temp();
        let h = FakeHost::default(); // daemon-reload unscripted -> error
        let o = ApplyOptions {
            systemctl: true,
            health_attempts: 1,
        };
        assert!(apply(&h, &l, &p, Mode::Fresh, &o).is_err());
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a/b.toml");
        write_atomic(&f, b"one", 0o600).unwrap();
        write_atomic(&f, b"two", 0o600).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "two");
        assert_eq!(fs::read_dir(d.path().join("a")).unwrap().count(), 1);
    }
}
