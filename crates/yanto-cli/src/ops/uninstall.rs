//! `yanto uninstall` (05 section 8): conservative by default, data kept
//! unless `--purge`.

use std::fs;
use std::path::Path;

use yanto_core::Config;

use super::host::Host;
use super::install::{cron_file, run_systemctl};
use super::layout::{Layout, UNIT_NAME, UNIT_PATH};
use super::prompt::Prompter;
use super::update;
use super::wizard::home_of;

pub const MANAGED_BEGIN: &str = "# BEGIN YANTO MANAGED";
pub const MANAGED_END: &str = "# END YANTO MANAGED";

#[derive(Debug, Clone, Default)]
pub struct UninstallOpts {
    pub purge: bool,
    /// `Some(true)` remove without asking, `Some(false)` keep, `None` ask.
    pub managed: Option<bool>,
    pub stop_projects: bool,
    pub remove_registry: bool,
    /// Assume yes to confirmations (does not by itself imply `purge`).
    pub yes: bool,
}

/// Remove the `# BEGIN/END YANTO MANAGED` block; returns the new text and
/// whether a block was found.
pub fn strip_managed_block(text: &str) -> (String, bool) {
    let mut out = Vec::new();
    let mut inside = false;
    let mut found = false;
    for line in text.lines() {
        let t = line.trim();
        if !inside && t == MANAGED_BEGIN {
            inside = true;
            found = true;
        } else if inside {
            if t == MANAGED_END {
                inside = false;
            }
        } else {
            out.push(line);
        }
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    (s, found)
}

fn decide(
    flag: Option<bool>,
    p: &mut Option<&mut dyn Prompter>,
    msg: &str,
) -> anyhow::Result<bool> {
    match (flag, p.as_deref_mut()) {
        (Some(v), _) => Ok(v),
        (None, Some(pr)) => pr.confirm(msg, true),
        (None, None) => Ok(true),
    }
}

fn remove_file_if(path: &Path, steps: &mut Vec<String>) -> anyhow::Result<bool> {
    if path.exists() {
        fs::remove_file(path)?;
        steps.push(format!("removed {}", path.display()));
        return Ok(true);
    }
    Ok(false)
}

pub fn run(
    host: &dyn Host,
    layout: &Layout,
    config_path: &Path,
    systemctl: bool,
    opts: &UninstallOpts,
    mut prompter: Option<&mut dyn Prompter>,
) -> anyhow::Result<Vec<String>> {
    let mut steps = Vec::new();
    let cfg = Config::load(&layout.path(config_path)).unwrap_or_default();

    // Validate purge confirmation up front, before anything is removed.
    if opts.purge && !opts.yes {
        match prompter.as_deref_mut() {
            Some(p) => {
                let a = p.text(
                    &format!(
                        "PURGE deletes {} and {}. Type 'yes' to confirm",
                        cfg.paths.data_dir.display(),
                        config_path.display()
                    ),
                    "",
                )?;
                if a.trim() != "yes" {
                    anyhow::bail!("purge not confirmed; nothing was changed");
                }
            }
            None => anyhow::bail!("--purge requires --yes (or an interactive confirmation)"),
        }
    }

    // 1. service + unit
    if systemctl {
        // Ignore failure: the unit may already be gone or never started.
        let _ = host.run("systemctl", &["disable", "--now", UNIT_NAME]);
        steps.push("systemctl disable --now yanto".into());
    }
    remove_file_if(&layout.path(UNIT_PATH), &mut steps)?;
    if systemctl {
        run_systemctl(host, &["daemon-reload"])?;
    }

    // 2. binary
    remove_file_if(&update::bin(layout), &mut steps)?;
    remove_file_if(&update::prev(layout), &mut steps)?;

    // 3. managed host files
    if decide(
        opts.managed,
        &mut prompter,
        "Remove /etc/cron.d/yanto (Cron Entries, scheduled backups)?",
    )? {
        remove_file_if(&cron_file(layout), &mut steps)?;
    }
    if let Some(home) = home_of(layout, &cfg.ssh.authorized_keys_user) {
        let ak = layout.path(home.join(".ssh/authorized_keys"));
        if let Ok(text) = fs::read_to_string(&ak) {
            let (new, found) = strip_managed_block(&text);
            if found
                && decide(
                    opts.managed,
                    &mut prompter,
                    "Remove the Yanto-managed block from authorized_keys (keeping it would leave SSH access granted)?",
                )?
            {
                super::install::write_atomic(&ak, new.as_bytes(), 0o600)?;
                steps.push(format!("removed managed block from {}", ak.display()));
            }
        }
    }
    let traefik_file = layout.path(cfg.ingress.traefik.dynamic_dir.join("yanto.yml"));
    if traefik_file.exists()
        && decide(
            opts.managed,
            &mut prompter,
            "Remove Yanto's Traefik file (its Hostnames stop routing)?",
        )?
    {
        remove_file_if(&traefik_file, &mut steps)?;
    }

    // 4. workloads: left alone unless asked.
    if opts.stop_projects {
        // TODO(M2): `docker compose down` per Project once Projects exist.
        steps.push("WARNING: --stop-projects is not implemented yet (needs Projects, M2); projects left running".into());
    }
    if opts.remove_registry {
        // TODO(M6): stop/remove the Hosted Registry container.
        steps.push(
            "WARNING: --remove-registry is not implemented yet (needs the Hosted Registry, M6)"
                .into(),
        );
    }

    // 5. purge
    if opts.purge {
        let data = layout.path(&cfg.paths.data_dir);
        if data.exists() {
            fs::remove_dir_all(&data)?;
            steps.push(format!("purged {}", cfg.paths.data_dir.display()));
        }
        let conf = layout.path(config_path);
        remove_file_if(&conf, &mut steps)?;
        if let Some(dir) = conf.parent() {
            let _ = fs::remove_dir(dir); // only if empty
        }
    } else {
        steps.push(format!(
            "kept {} and {} (use --purge to delete)",
            cfg.paths.data_dir.display(),
            config_path.display()
        ));
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::host::FakeHost;
    use crate::ops::install::{ApplyOptions, Mode, apply};
    use crate::ops::prompt::Scripted;
    use crate::ops::render::fixtures::params;

    fn installed() -> (tempfile::TempDir, Layout) {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path());
        fs::create_dir_all(l.path("/etc")).unwrap();
        fs::write(
            l.path("/etc/passwd"),
            "deploy:x:1000:1000::/home/deploy:/bin/sh\n",
        )
        .unwrap();
        let mut p = params();
        p.start = false;
        apply(
            &FakeHost::default(),
            &l,
            &p,
            Mode::Fresh,
            &ApplyOptions {
                systemctl: false,
                health_attempts: 1,
            },
        )
        .unwrap();
        fs::create_dir_all(l.path("/usr/local/bin")).unwrap();
        fs::write(l.path("/usr/local/bin/yanto"), b"bin").unwrap();
        fs::create_dir_all(l.path("/etc/cron.d")).unwrap();
        fs::write(l.path("/etc/cron.d/yanto"), b"* * * * * root true").unwrap();
        fs::create_dir_all(l.path("/home/deploy/.ssh")).unwrap();
        fs::write(
            l.path("/home/deploy/.ssh/authorized_keys"),
            "ssh-ed25519 AAAA user-key\n# BEGIN YANTO MANAGED\nssh-ed25519 BBBB managed\n# END YANTO MANAGED\n",
        )
        .unwrap();
        (d, l)
    }

    #[test]
    fn strip_block_cases() {
        let (s, f) = strip_managed_block("a\n# BEGIN YANTO MANAGED\nx\n# END YANTO MANAGED\nb\n");
        assert!(f);
        assert_eq!(s, "a\nb\n");
        let (s, f) = strip_managed_block("a\nb\n");
        assert!(!f);
        assert_eq!(s, "a\nb\n");
        assert_eq!(strip_managed_block("").0, "");
    }

    #[test]
    fn default_keeps_data_removes_the_rest() {
        let (_d, l) = installed();
        let opts = UninstallOpts {
            yes: true,
            ..Default::default()
        };
        let steps = run(
            &FakeHost::default(),
            &l,
            Path::new("/etc/yanto/config.toml"),
            false,
            &opts,
            None,
        )
        .unwrap();
        assert!(!l.path(UNIT_PATH).exists());
        assert!(!l.path("/usr/local/bin/yanto").exists());
        assert!(!l.path("/etc/cron.d/yanto").exists());
        let ak = fs::read_to_string(l.path("/home/deploy/.ssh/authorized_keys")).unwrap();
        assert!(ak.contains("user-key") && !ak.contains("managed"));
        assert!(l.path("/var/lib/yanto/yanto.db").exists());
        assert!(l.path("/etc/yanto/config.toml").exists());
        assert!(steps.last().unwrap().starts_with("kept"));
    }

    #[test]
    fn keep_managed_leaves_cron_and_keys() {
        let (_d, l) = installed();
        let opts = UninstallOpts {
            yes: true,
            managed: Some(false),
            ..Default::default()
        };
        run(
            &FakeHost::default(),
            &l,
            Path::new("/etc/yanto/config.toml"),
            false,
            &opts,
            None,
        )
        .unwrap();
        assert!(l.path("/etc/cron.d/yanto").exists());
        assert!(
            fs::read_to_string(l.path("/home/deploy/.ssh/authorized_keys"))
                .unwrap()
                .contains("managed")
        );
    }

    #[test]
    fn purge_needs_confirmation() {
        let (_d, l) = installed();
        let opts = UninstallOpts {
            purge: true,
            ..Default::default()
        };
        let c = Path::new("/etc/yanto/config.toml");
        // no yes, no prompter
        assert!(run(&FakeHost::default(), &l, c, false, &opts, None).is_err());
        assert!(l.path("/var/lib/yanto").exists());
        // wrong answer
        let mut s = Scripted::default();
        s.texts.push_back("no".into());
        assert!(run(&FakeHost::default(), &l, c, false, &opts, Some(&mut s)).is_err());
        assert!(l.path("/var/lib/yanto").exists());
        assert!(l.path(UNIT_PATH).exists(), "nothing changed on refusal");
    }

    #[test]
    fn purge_with_yes_deletes_data_and_config() {
        let (_d, l) = installed();
        let opts = UninstallOpts {
            purge: true,
            yes: true,
            ..Default::default()
        };
        run(
            &FakeHost::default(),
            &l,
            Path::new("/etc/yanto/config.toml"),
            false,
            &opts,
            None,
        )
        .unwrap();
        assert!(!l.path("/var/lib/yanto").exists());
        assert!(!l.path("/etc/yanto").exists());
    }

    #[test]
    fn systemctl_disable_then_reload() {
        let (_d, l) = installed();
        let mut h = FakeHost::default();
        h.respond(
            "systemctl daemon-reload",
            crate::ops::host::CmdOutput::ok(""),
        );
        let opts = UninstallOpts {
            yes: true,
            ..Default::default()
        };
        run(
            &h,
            &l,
            Path::new("/etc/yanto/config.toml"),
            true,
            &opts,
            None,
        )
        .unwrap();
        assert_eq!(
            *h.calls.borrow(),
            vec!["systemctl disable --now yanto", "systemctl daemon-reload"]
        );
    }
}
