//! `yanto uninstall` (05 section 8): conservative by default, data kept
//! unless `--purge`.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use yanto_core::Config;
use yanto_host::fsutil::DirHandle;

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

/// Project slugs, read from the Project directories (the DB may already be unreadable). Names
/// that are not a valid slug are skipped so nothing odd reaches `docker` argv.
fn project_slugs(root: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| yanto_core::deploy::validate::slug(n).is_ok())
        .collect();
    out.sort();
    out
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
        // `~/.ssh` belongs to that user: go through an O_NOFOLLOW directory handle so a planted
        // symlink cannot redirect this root write, and keep the file's owner.
        let ssh = layout.path(home.join(".ssh"));
        if let Ok(Some(dir)) = DirHandle::open(&ssh)
            && let Ok(Some((bytes, meta))) = dir.read_file("authorized_keys")
        {
            let (new, found) = strip_managed_block(&String::from_utf8_lossy(&bytes));
            if found
                && decide(
                    opts.managed,
                    &mut prompter,
                    "Remove the Yanto-managed block from authorized_keys (keeping it would leave SSH access granted)?",
                )?
            {
                dir.write_atomic(
                    ".authorized_keys.yanto.tmp",
                    "authorized_keys",
                    new.as_bytes(),
                    0o600,
                    Some((meta.uid(), meta.gid())),
                )?;
                steps.push(format!(
                    "removed managed block from {}",
                    ssh.join("authorized_keys").display()
                ));
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
    // Runs before `--purge`, which deletes the Project directories these are found from.
    if opts.stop_projects {
        for slug in project_slugs(&layout.path(cfg.paths.data_dir.join("projects"))) {
            // `-p` alone is enough: Compose finds the stack by its project label. No `--volumes`.
            let line = format!("docker compose -p {slug} down");
            match host.run("docker", &["compose", "-p", &slug, "down"]) {
                Ok(o) if o.success => steps.push(line),
                Ok(o) => steps.push(format!("WARNING: {line} failed: {}", o.stderr.trim())),
                Err(e) => steps.push(format!("WARNING: {line} failed: {e}")),
            }
        }
    }
    if opts.remove_registry {
        let name = yanto_core::hosted_registry::DEFAULT_CONTAINER;
        let line = format!("docker rm -f {name}");
        match host.run("docker", &["rm", "-f", name]) {
            Ok(o) if o.success => steps.push(line),
            Ok(o) => steps.push(format!("WARNING: {line} failed: {}", o.stderr.trim())),
            Err(e) => steps.push(format!("WARNING: {line} failed: {e}")),
        }
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
    fn a_symlinked_authorized_keys_is_not_followed() {
        let (_d, l) = installed();
        let ak = l.path("/home/deploy/.ssh/authorized_keys");
        let elsewhere = l.path("/etc/elsewhere");
        fs::rename(&ak, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &ak).unwrap();
        let before = fs::read_to_string(&elsewhere).unwrap();
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
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), before);
        assert!(fs::symlink_metadata(&ak).unwrap().file_type().is_symlink());
        assert!(!steps.iter().any(|s| s.contains("managed block")));
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
    fn stop_projects_and_remove_registry_run_docker() {
        let (_d, l) = installed();
        let projects = l.path("/var/lib/yanto/projects");
        fs::create_dir_all(projects.join("web")).unwrap();
        fs::create_dir_all(projects.join("-oops")).unwrap();
        let mut h = FakeHost::default();
        h.respond(
            "docker compose -p web down",
            crate::ops::host::CmdOutput::ok(""),
        );
        h.respond(
            "docker rm -f yanto-registry",
            crate::ops::host::CmdOutput::ok(""),
        );
        let opts = UninstallOpts {
            yes: true,
            stop_projects: true,
            remove_registry: true,
            ..Default::default()
        };
        let steps = run(
            &h,
            &l,
            Path::new("/etc/yanto/config.toml"),
            false,
            &opts,
            None,
        )
        .unwrap();
        assert_eq!(
            *h.calls.borrow(),
            vec!["docker compose -p web down", "docker rm -f yanto-registry"]
        );
        assert!(!steps.iter().any(|s| s.contains("WARNING")), "{steps:?}");
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
