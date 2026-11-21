//! `yanto install` wizard (docs/rewrite/05 section 3).

use std::path::Path;

use anyhow::{Context, bail};
use yanto_core::Config;

use crate::ops::checks::{self, DaemonInput, PrereqInput, Status};
use crate::ops::host::{Host, RealHost};
use crate::ops::install::{ApplyOptions, Mode, apply, existing_install};
use crate::ops::layout::{Layout, UNIT_PATH};
use crate::ops::params::InstallParams;
use crate::ops::prompt::{InquirePrompter, Prompter};
use crate::ops::wizard::{InstallArgs, resolve};

pub use crate::ops::wizard::InstallArgs as Args;

pub fn run(config_path: &Path, args: &InstallArgs) -> anyhow::Result<()> {
    let layout = Layout::from_root(args.root.as_deref());
    let host = RealHost;
    let mut inquire = InquirePrompter;
    let prompter: Option<&mut dyn Prompter> = if args.non_interactive {
        None
    } else {
        Some(&mut inquire)
    };
    execute(&host, &layout, config_path, args, prompter)
}

/// Exit code 3 for failed prerequisites (05 section 10).
fn prereq_failed() -> ! {
    std::process::exit(3)
}

pub fn execute(
    host: &dyn Host,
    layout: &Layout,
    config_path: &Path,
    args: &InstallArgs,
    mut prompter: Option<&mut dyn Prompter>,
) -> anyhow::Result<()> {
    let existing = existing_install(layout, config_path);
    let existing_cfg = Config::load(&layout.path(config_path)).ok();

    let mode = if !existing {
        Mode::Fresh
    } else if args.reconfigure {
        Mode::Reconfigure
    } else if let Some(p) = prompter.as_deref_mut() {
        match p.select(
            "An existing Yanto install was found",
            &[
                "Reconfigure",
                "Upgrade only (keep config, re-render unit, restart)",
                "Abort",
            ],
        )? {
            0 => Mode::Reconfigure,
            1 => Mode::UpgradeOnly,
            _ => bail!("aborted"),
        }
    } else if args.force {
        Mode::Reconfigure
    } else {
        bail!(
            "an existing install was found; pass --reconfigure or --force (or run interactively)"
        );
    };
    if existing {
        println!("Existing install detected: mode = {mode:?}");
    }

    // Prerequisites run before any prompt, using flag values or defaults.
    let base = existing_cfg.clone().unwrap_or_default();
    let probe_bind = args
        .bind
        .as_deref()
        .and_then(|b| b.parse().ok())
        .or_else(|| base.server.bind.parse().ok())
        .context("invalid --bind")?;
    let probe_port = args.port.unwrap_or(base.server.port);
    let input = PrereqInput {
        bind: probe_bind,
        port: probe_port,
        port_is_ours: existing_cfg.as_ref().is_some_and(|c| {
            c.server.port == probe_port && c.server.bind == probe_bind.to_string()
        }),
        data_dir: args
            .data_dir
            .clone()
            .unwrap_or_else(|| base.paths.data_dir.clone()),
        docker_socket: base.docker.socket.clone(),
        registry: crate::ops::wizard::tri(args.enable_registry, args.no_enable_registry)
            .unwrap_or(false),
    };
    let pre = checks::prereq_checks(host, layout, &input);
    println!("Prerequisite checks:\n{}", checks::render_table(&pre));
    if checks::worst(&pre) == Status::Fail {
        if args.skip_prereq_checks {
            println!("Continuing despite failed checks (--skip-prereq-checks).");
        } else {
            eprintln!(
                "Prerequisites failed. Fix the items above (Yanto never installs them) and re-run, or `yanto doctor` to re-check."
            );
            prereq_failed();
        }
    }

    let (params, warnings) = if mode == Mode::UpgradeOnly {
        let cfg = existing_cfg.context("cannot upgrade-only: existing config is unreadable")?;
        let mut p = InstallParams::from_config(&cfg, config_path)?;
        p.hardened = args.hardened
            || std::fs::read_to_string(layout.path(UNIT_PATH))
                .is_ok_and(|u| u.contains("CapabilityBoundingSet="));
        (p, vec![])
    } else {
        let r = resolve(
            args,
            host,
            layout,
            config_path,
            existing_cfg.as_ref(),
            prompter,
        )?;
        (r.params, r.warnings)
    };
    for w in &warnings {
        println!("warning: {w}");
    }

    let daemons = checks::daemon_checks(
        host,
        &DaemonInput {
            cloudflared_unit: params
                .cloudflared_selected()
                .then(|| params.cloudflared_unit.clone()),
            traefik_unit: params
                .traefik_selected()
                .then(|| params.traefik_unit.clone()),
            frps_unit: params.frps.then(|| params.frps_unit.clone()),
        },
    );
    if !daemons.is_empty() && layout.is_system() {
        println!(
            "Host Daemon checks (warnings only):\n{}",
            checks::render_table(&daemons)
        );
    }

    let systemctl = layout.is_system();
    if !systemctl {
        println!("note: --root is set; systemctl calls are skipped and the service is not started");
    }
    let report = apply(
        host,
        layout,
        &params,
        mode,
        &ApplyOptions {
            systemctl,
            health_attempts: 30,
        },
    )?;
    for s in &report.steps {
        println!("  - {s}");
    }

    println!("\nYanto is installed.");
    println!("  URL:     http://{}:{}", params.bind, params.port);
    println!("  config:  {}", params.config_path.display());
    println!("  unit:    yanto.service");
    match report.setup_code {
        Some(code) => {
            println!("\n  SETUP CODE (shown once): {code}");
            println!(
                "  It is a live credential until used. Clear your terminal scrollback and do not paste it into tickets or chat."
            );
        }
        None if mode == Mode::Fresh => {
            // TODO(M1): the auth module issues the code; until then point at the command.
            println!(
                "\n  No setup code was issued here. Run `sudo yanto setup-code` to create one."
            );
        }
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use crate::ops::host::FakeHost;
    use clap::Parser;

    fn args(extra: &[&str]) -> InstallArgs {
        let mut v = vec!["yanto", "install"];
        v.extend_from_slice(extra);
        match Cli::try_parse_from(v).unwrap().command {
            Command::Install(a) => a,
            _ => unreachable!(),
        }
    }

    fn root_with_users() -> (tempfile::TempDir, Layout) {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path());
        std::fs::create_dir_all(l.path("/etc")).unwrap();
        std::fs::create_dir_all(l.path("/run/systemd/system")).unwrap();
        std::fs::write(l.path("/etc/os-release"), "ID=debian\nVERSION_ID=\"12\"\n").unwrap();
        std::fs::write(
            l.path("/etc/passwd"),
            "deploy:x:1000:1000::/home/deploy:/bin/sh\n",
        )
        .unwrap();
        (d, l)
    }

    #[test]
    fn non_interactive_install_into_temp_root_end_to_end() {
        let (_d, l) = root_with_users();
        let a = args(&[
            "--non-interactive",
            "--ingress",
            "traefik",
            "--no-enable-registry",
        ]);
        execute(
            &FakeHost::healthy_host(),
            &l,
            Path::new("/etc/yanto/config.toml"),
            &a,
            None,
        )
        .unwrap();
        let cfg = Config::load(&l.path("/etc/yanto/config.toml")).unwrap();
        assert_eq!(cfg.ingress.providers, vec!["traefik"]);
        assert_eq!(cfg.ssh.authorized_keys_user, "deploy");
        assert!(l.path("/etc/systemd/system/yanto.service").exists());
    }

    #[test]
    fn existing_install_refused_without_reconfigure_then_allowed() {
        let (_d, l) = root_with_users();
        let c = Path::new("/etc/yanto/config.toml");
        let h = FakeHost::healthy_host();
        execute(&h, &l, c, &args(&["--yes"]), None).unwrap();
        let e = execute(&h, &l, c, &args(&["--yes"]), None).unwrap_err();
        assert!(e.to_string().contains("--reconfigure"));
        execute(
            &h,
            &l,
            c,
            &args(&["--yes", "--reconfigure", "--port", "9500"]),
            None,
        )
        .unwrap();
        assert_eq!(Config::load(&l.path(c)).unwrap().server.port, 9500);
    }

    #[test]
    fn reconfigure_reuses_existing_values_as_defaults() {
        let (_d, l) = root_with_users();
        let c = Path::new("/etc/yanto/config.toml");
        let h = FakeHost::healthy_host();
        execute(
            &h,
            &l,
            c,
            &args(&["--yes", "--port", "9400", "--ingress", "traefik"]),
            None,
        )
        .unwrap();
        execute(&h, &l, c, &args(&["--yes", "--reconfigure"]), None).unwrap();
        let cfg = Config::load(&l.path(c)).unwrap();
        assert_eq!(cfg.server.port, 9400);
        assert_eq!(cfg.ingress.providers, vec!["traefik"]);
    }

    #[test]
    fn upgrade_only_rerenders_unit_but_keeps_config() {
        let (_d, l) = root_with_users();
        let c = Path::new("/etc/yanto/config.toml");
        let h = FakeHost::healthy_host();
        execute(&h, &l, c, &args(&["--yes", "--port", "9400"]), None).unwrap();
        let mut s = crate::ops::prompt::Scripted::default();
        s.selects.push_back(1);
        execute(&h, &l, c, &args(&["--port", "1111"]), Some(&mut s)).unwrap();
        assert_eq!(Config::load(&l.path(c)).unwrap().server.port, 9400);
    }
}
