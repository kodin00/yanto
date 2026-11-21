//! `yanto doctor`: re-run the prerequisite, Host Daemon and path checks.

use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Args;
use yanto_core::Config;

use crate::ops::checks::{self, Check, DaemonInput, PrereqInput, Status};
use crate::ops::host::{Host, RealHost};
use crate::ops::layout::{Layout, UNIT_PATH};
use crate::ops::params::InstallParams;
use crate::ops::render::{read_write_paths, ssh_dir_for, unit_read_write_paths};
use crate::ops::wizard::home_of;

#[derive(Debug, Clone, Args, Default)]
pub struct DoctorArgs {
    /// Operate under this directory prefix instead of /
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
}

pub fn run(config_path: &Path, args: &DoctorArgs) -> anyhow::Result<()> {
    let layout = Layout::from_root(args.root.as_deref());
    let cfg = Config::load(&layout.path(config_path)).with_context(|| {
        format!(
            "loading {} (is Yanto installed? run `yanto install`)",
            config_path.display()
        )
    })?;
    let all = doctor_checks(&RealHost, &layout, &cfg, config_path)?;
    println!("{}", checks::render_table(&all));
    if checks::worst(&all) == Status::Fail {
        std::process::exit(3);
    }
    Ok(())
}

pub fn doctor_checks(
    host: &dyn Host,
    layout: &Layout,
    cfg: &Config,
    config_path: &Path,
) -> anyhow::Result<Vec<Check>> {
    let p = InstallParams::from_config(cfg, config_path)?;
    let mut out = checks::prereq_checks(
        host,
        layout,
        &PrereqInput {
            bind: p.bind,
            port: p.port,
            port_is_ours: true,
            data_dir: p.data_dir.clone(),
            docker_socket: cfg.docker.socket.clone(),
            registry: p.registry,
        },
    );
    out.extend(checks::daemon_checks(
        host,
        &DaemonInput {
            cloudflared_unit: p.cloudflared_selected().then(|| p.cloudflared_unit.clone()),
            traefik_unit: p.traefik_selected().then(|| p.traefik_unit.clone()),
            frps_unit: p.frps.then(|| p.frps_unit.clone()),
        },
    ));
    out.push(unit_paths_check(layout, &p));
    out.push(service_check(host, &p));
    Ok(out)
}

fn mk(name: &str, status: Status, detail: String, hint: Option<&str>) -> Check {
    Check {
        name: name.into(),
        status,
        detail,
        hint: hint.map(str::to_string),
    }
}

/// OQ-11: name configured paths missing from the unit's `ReadWritePaths`.
fn unit_paths_check(layout: &Layout, p: &InstallParams) -> Check {
    let Ok(unit) = std::fs::read_to_string(layout.path(UNIT_PATH)) else {
        return mk(
            "unit paths",
            Status::Warn,
            format!("{UNIT_PATH} not readable"),
            Some("run `yanto install --reconfigure`"),
        );
    };
    let have = unit_read_write_paths(&unit);
    let home = home_of(layout, &p.ssh_user);
    let missing: Vec<String> = read_write_paths(p, ssh_dir_for(home.as_deref()).as_deref())
        .into_iter()
        .map(|s| s.trim_start_matches('-').to_string())
        .filter(|s| !have.contains(s))
        .collect();
    if missing.is_empty() {
        mk(
            "unit paths",
            Status::Ok,
            "ReadWritePaths covers every configured path".into(),
            None,
        )
    } else {
        mk(
            "unit paths",
            Status::Warn,
            format!("not in ReadWritePaths: {}", missing.join(", ")),
            Some(
                "writes there fail with EROFS; run `sudo yanto install --reconfigure` to re-render the unit",
            ),
        )
    }
}

fn service_check(host: &dyn Host, p: &InstallParams) -> Check {
    if !host.succeeds("systemctl", &["is-active", "--quiet", "yanto.service"]) {
        return mk(
            "service",
            Status::Warn,
            "yanto.service is not active".into(),
            Some("systemctl status yanto; journalctl -u yanto -n 50"),
        );
    }
    if host.healthy(p.bind, p.port) {
        mk(
            "service",
            Status::Ok,
            format!("active, /healthz ok on {}:{}", p.bind, p.port),
            None,
        )
    } else {
        mk(
            "service",
            Status::Fail,
            format!("active but /healthz failed on {}:{}", p.bind, p.port),
            Some("journalctl -u yanto -n 50"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::host::{CmdOutput, FakeHost};
    use crate::ops::render::{fixtures::params, render_unit};

    fn setup() -> (tempfile::TempDir, Layout, Config) {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path());
        for dir in ["/etc/systemd/system", "/run/systemd/system"] {
            std::fs::create_dir_all(l.path(dir)).unwrap();
        }
        std::fs::write(l.path("/etc/os-release"), "ID=debian\nVERSION_ID=12\n").unwrap();
        std::fs::write(
            l.path("/etc/passwd"),
            "deploy:x:1000:1000::/home/deploy:/bin/sh\n",
        )
        .unwrap();
        (d, l, Config::default())
    }

    #[test]
    fn detects_unit_missing_a_configured_path() {
        let (_d, l, mut cfg) = setup();
        let mut p = params();
        p.start = false;
        std::fs::write(
            l.path(UNIT_PATH),
            render_unit(&p, Some(Path::new("/home/deploy/.ssh"))),
        )
        .unwrap();
        let mut h = FakeHost::healthy_host();
        h.respond(
            "systemctl is-active --quiet yanto.service",
            CmdOutput::ok(""),
        );
        let ok = doctor_checks(&h, &l, &cfg, Path::new("/etc/yanto/config.toml")).unwrap();
        assert_eq!(
            ok.iter().find(|c| c.name == "unit paths").unwrap().status,
            Status::Ok
        );

        cfg.ingress.traefik.dynamic_dir = "/opt/traefik/dyn".into();
        let bad = doctor_checks(&h, &l, &cfg, Path::new("/etc/yanto/config.toml")).unwrap();
        let c = bad.iter().find(|c| c.name == "unit paths").unwrap();
        assert_eq!(c.status, Status::Warn);
        assert!(c.detail.contains("/opt/traefik/dyn"));
    }

    #[test]
    fn service_down_and_unhealthy_states() {
        let (_d, l, cfg) = setup();
        let h = FakeHost::healthy_host();
        let c = doctor_checks(&h, &l, &cfg, Path::new("/c")).unwrap();
        assert_eq!(
            c.iter().find(|c| c.name == "service").unwrap().status,
            Status::Warn
        );
        let mut h = FakeHost::healthy_host();
        h.healthy = false;
        h.respond(
            "systemctl is-active --quiet yanto.service",
            CmdOutput::ok(""),
        );
        let c = doctor_checks(&h, &l, &cfg, Path::new("/c")).unwrap();
        assert_eq!(
            c.iter().find(|c| c.name == "service").unwrap().status,
            Status::Fail
        );
    }
}
