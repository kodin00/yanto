//! Flag + prompt + default resolution for `yanto install` (05 section 3.2).
//! A flag given on the command line suppresses its prompt; `--non-interactive`
//! never prompts and fails if a value has no default.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Args;
use yanto_core::Config;

use super::host::Host;
use super::layout::{Layout, PASSWD};
use super::params::{Ingress, InstallParams, PasswdEntry, parse_passwd};
use super::prompt::{Prompter, ask_validated};

#[derive(Debug, Clone, Args, Default)]
pub struct InstallArgs {
    /// Never prompt: use flags, then defaults; fail if a value has none
    #[arg(long, visible_alias = "yes")]
    pub non_interactive: bool,
    /// Bind address of the dashboard [default: 127.0.0.1]
    #[arg(long)]
    pub bind: Option<String>,
    /// Allow a non-loopback bind in --non-interactive mode
    #[arg(long)]
    pub allow_public_bind: bool,
    /// Dashboard port [default: 9197]
    #[arg(long)]
    pub port: Option<u16>,
    /// Public URL (drives cookie Secure + CSRF origin check)
    #[arg(long)]
    pub public_url: Option<String>,
    /// Data directory [default: /var/lib/yanto]
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    /// Unix user whose authorized_keys Yanto manages
    #[arg(long)]
    pub ssh_user: Option<String>,
    /// Expect the Hosted Registry (only checks 127.0.0.1:5000)
    #[arg(long, overrides_with = "no_enable_registry")]
    pub enable_registry: bool,
    #[arg(long, overrides_with = "enable_registry")]
    pub no_enable_registry: bool,
    /// Ingress Providers to expect (comma-separated or repeated)
    #[arg(long, value_enum, value_delimiter = ',')]
    pub ingress: Vec<Ingress>,
    /// Traefik dynamic config dir [default: /etc/traefik/dynamic]
    #[arg(long)]
    pub traefik_dynamic_dir: Option<PathBuf>,
    /// Traefik cert resolver name [default: letsencrypt]
    #[arg(long)]
    pub traefik_cert_resolver: Option<String>,
    #[arg(long)]
    pub traefik_unit: Option<String>,
    #[arg(long)]
    pub cloudflared_unit: Option<String>,
    /// Expect frps
    #[arg(long, overrides_with = "no_frps")]
    pub frps: bool,
    #[arg(long, overrides_with = "frps")]
    pub no_frps: bool,
    /// frps config path [default: /etc/frp/frps.toml]
    #[arg(long)]
    pub frps_config: Option<PathBuf>,
    #[arg(long)]
    pub frps_unit: Option<String>,
    /// Enable and start the service after install
    #[arg(long, overrides_with = "no_start")]
    pub start: bool,
    /// Write everything but do not enable/start the service
    #[arg(long, overrides_with = "start")]
    pub no_start: bool,
    /// Add the optional capability-bounding hardening profile to the unit
    #[arg(long)]
    pub hardened: bool,
    /// Reconfigure an existing install (reuses current values as defaults)
    #[arg(long)]
    pub reconfigure: bool,
    /// Proceed over an existing install without asking
    #[arg(long)]
    pub force: bool,
    /// Continue even if a prerequisite check fails (discouraged)
    #[arg(long)]
    pub skip_prereq_checks: bool,
    /// Operate under this directory prefix instead of / (tests, dry runs).
    /// Implies no systemctl calls and no service start.
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
}

pub fn tri(yes: bool, no: bool) -> Option<bool> {
    match (yes, no) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

pub struct Resolved {
    pub params: InstallParams,
    pub warnings: Vec<String>,
}

pub fn validate_bind(s: &str) -> Result<IpAddr, String> {
    s.parse().map_err(|_| format!("{s:?} is not an IP address"))
}

pub fn validate_port(s: &str) -> Result<u16, String> {
    match s.parse::<u16>() {
        Ok(p) if p > 0 => Ok(p),
        _ => Err(format!("{s:?} is not a port (1-65535)")),
    }
}

pub fn validate_abs(s: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(s);
    if p.is_absolute() {
        Ok(p)
    } else {
        Err(format!("{s:?} must be an absolute path"))
    }
}

pub fn validate_url(s: &str) -> Result<String, String> {
    if s.is_empty() || s.starts_with("http://") || s.starts_with("https://") {
        Ok(s.to_string())
    } else {
        Err("must be empty or start with http:// or https://".into())
    }
}

/// `$SUDO_USER` if usable, else the first regular user (uid 1000..65533).
pub fn default_ssh_user(sudo_user: Option<&str>, passwd: &[PasswdEntry]) -> Option<String> {
    if let Some(u) = sudo_user
        && u != "root"
        && passwd.iter().any(|e| e.name == u)
    {
        return Some(u.to_string());
    }
    passwd
        .iter()
        .find(|e| (1000..65534).contains(&e.uid))
        .map(|e| e.name.clone())
}

fn read_passwd(layout: &Layout) -> Vec<PasswdEntry> {
    std::fs::read_to_string(layout.path(PASSWD))
        .map(|t| parse_passwd(&t))
        .unwrap_or_default()
}

/// Home directory of a configured SSH user, if present in passwd.
pub fn home_of(layout: &Layout, user: &str) -> Option<PathBuf> {
    read_passwd(layout)
        .into_iter()
        .find(|e| e.name == user)
        .map(|e| e.home)
}

/// Merge flags, prompts and defaults. `existing` supplies defaults on
/// reconfigure. `prompter` is `Some` only in interactive mode.
pub fn resolve(
    args: &InstallArgs,
    host: &dyn Host,
    layout: &Layout,
    config_path: &Path,
    existing: Option<&Config>,
    mut prompter: Option<&mut dyn Prompter>,
) -> anyhow::Result<Resolved> {
    let mut warnings = Vec::new();
    let base = existing.cloned().unwrap_or_default();
    let passwd = read_passwd(layout);

    // bind
    let bind_default = base.server.bind.clone();
    let bind: IpAddr = match (&args.bind, prompter.as_deref_mut()) {
        (Some(v), _) => validate_bind(v).map_err(anyhow::Error::msg)?,
        (None, Some(p)) => ask_validated(p, "Bind address", &bind_default, validate_bind)?,
        (None, None) => validate_bind(&bind_default).map_err(anyhow::Error::msg)?,
    };
    if !bind.is_loopback() {
        if prompter.is_none() && !args.allow_public_bind {
            bail!(
                "non-loopback bind {bind} requires --allow-public-bind (the dashboard is plain HTTP; prefer an Ingress Provider)"
            );
        }
        warnings.push(format!(
            "bind {bind} is not loopback: the dashboard is plain HTTP on that interface; put it behind an Ingress Provider instead"
        ));
    }

    // port (re-asks when busy, unless it is the existing install's own port)
    let port_ours =
        |p: u16| existing.is_some_and(|c| c.server.port == p && c.server.bind == bind.to_string());
    let port: u16 = match (args.port, prompter.as_deref_mut()) {
        (Some(v), _) => validate_port(&v.to_string()).map_err(anyhow::Error::msg)?,
        (None, Some(p)) => ask_validated(p, "Port", &base.server.port.to_string(), |s| {
            let port = validate_port(s)?;
            if !port_ours(port) && !host.port_free(bind, port) {
                return Err(format!("port {port} is already in use"));
            }
            Ok(port)
        })?,
        (None, None) => base.server.port,
    };

    let public_url = match (&args.public_url, prompter.as_deref_mut()) {
        (Some(v), _) => validate_url(v).map_err(anyhow::Error::msg)?,
        (None, Some(p)) => ask_validated(
            p,
            "Public URL (optional, e.g. https://yanto.example.com)",
            &base.server.public_url,
            validate_url,
        )?,
        (None, None) => base.server.public_url.clone(),
    };

    let data_dir = match (&args.data_dir, prompter.as_deref_mut()) {
        (Some(v), _) => validate_abs(&v.to_string_lossy()).map_err(anyhow::Error::msg)?,
        (None, Some(p)) => ask_validated(
            p,
            "Data directory",
            &base.paths.data_dir.to_string_lossy(),
            validate_abs,
        )?,
        (None, None) => base.paths.data_dir.clone(),
    };

    // ssh user
    let ssh_default = match existing {
        Some(c) => Some(c.ssh.authorized_keys_user.clone()),
        None => default_ssh_user(host.env("SUDO_USER").as_deref(), &passwd),
    };
    let user_exists = |u: &str| -> Result<String, String> {
        if passwd.iter().any(|e| e.name == u) {
            Ok(u.to_string())
        } else {
            Err(format!("user {u:?} not found in /etc/passwd"))
        }
    };
    let ssh_user = match (&args.ssh_user, prompter.as_deref_mut()) {
        (Some(v), _) => user_exists(v).map_err(anyhow::Error::msg)?,
        (None, Some(p)) => ask_validated(
            p,
            "Unix user for Authorized Keys",
            ssh_default.as_deref().unwrap_or(""),
            user_exists,
        )?,
        (None, None) => ssh_default
            .clone()
            .context("cannot derive a Unix user for Authorized Keys; pass --ssh-user")?,
    };
    if ssh_user == "root" {
        warnings.push("ssh user is root: Authorized Keys will grant root SSH access".into());
    }

    let registry = match (
        tri(args.enable_registry, args.no_enable_registry),
        prompter.as_deref_mut(),
    ) {
        (Some(v), _) => v,
        (None, Some(p)) => p.confirm("Enable Hosted Registry?", base.registry.enabled)?,
        (None, None) => base.registry.enabled,
    };

    // ingress
    let ingress: Vec<Ingress> = if !args.ingress.is_empty() {
        let mut v: Vec<Ingress> = args
            .ingress
            .iter()
            .copied()
            .filter(|i| *i != Ingress::None)
            .collect();
        v.dedup();
        v
    } else if let Some(p) = prompter.as_deref_mut() {
        let opts = ["cloudflare-tunnel", "traefik"];
        let defaults: Vec<usize> = base
            .ingress
            .providers
            .iter()
            .filter_map(|n| opts.iter().position(|o| o == n))
            .filter(|_| existing.is_some())
            .collect();
        p.multiselect("Ingress Providers to expect", &opts, &defaults)?
            .into_iter()
            .map(|i| {
                if i == 0 {
                    Ingress::CloudflareTunnel
                } else {
                    Ingress::Traefik
                }
            })
            .collect()
    } else if existing.is_some() {
        base.ingress
            .providers
            .iter()
            .filter_map(|n| match n.as_str() {
                "cloudflare-tunnel" => Some(Ingress::CloudflareTunnel),
                "traefik" => Some(Ingress::Traefik),
                _ => None,
            })
            .collect()
    } else {
        vec![]
    };
    let traefik = ingress.contains(&Ingress::Traefik);

    let traefik_dynamic_dir = match (&args.traefik_dynamic_dir, traefik, prompter.as_deref_mut()) {
        (Some(v), _, _) => validate_abs(&v.to_string_lossy()).map_err(anyhow::Error::msg)?,
        (None, true, Some(p)) => ask_validated(
            p,
            "Traefik dynamic config dir",
            &base.ingress.traefik.dynamic_dir.to_string_lossy(),
            validate_abs,
        )?,
        _ => base.ingress.traefik.dynamic_dir.clone(),
    };
    if traefik && !layout.path(&traefik_dynamic_dir).is_dir() {
        warnings.push(format!(
            "traefik dynamic dir {} does not exist yet",
            traefik_dynamic_dir.display()
        ));
    }
    let traefik_cert_resolver = match (
        &args.traefik_cert_resolver,
        traefik,
        prompter.as_deref_mut(),
    ) {
        (Some(v), _, _) => v.clone(),
        (None, true, Some(p)) => ask_validated(
            p,
            "Traefik cert resolver",
            &base.ingress.traefik.cert_resolver,
            |s| {
                if s.is_empty() {
                    Err("required".into())
                } else {
                    Ok(s.to_string())
                }
            },
        )?,
        _ => base.ingress.traefik.cert_resolver.clone(),
    };

    let frps = match (tri(args.frps, args.no_frps), prompter.as_deref_mut()) {
        (Some(v), _) => v,
        (None, Some(p)) => p.confirm("Expect frps (FRP server) on this Host?", base.frp.enabled)?,
        (None, None) => base.frp.enabled,
    };
    let frps_config = match (&args.frps_config, frps, prompter) {
        (Some(v), _, _) => validate_abs(&v.to_string_lossy()).map_err(anyhow::Error::msg)?,
        (None, true, Some(p)) => ask_validated(
            p,
            "frps config path",
            &base.frp.config_path.to_string_lossy(),
            validate_abs,
        )?,
        _ => base.frp.config_path.clone(),
    };

    let params = InstallParams {
        config_path: config_path.to_path_buf(),
        bind,
        port,
        public_url,
        data_dir,
        ssh_user,
        registry,
        ingress,
        traefik_dynamic_dir,
        traefik_cert_resolver,
        traefik_unit: args
            .traefik_unit
            .clone()
            .unwrap_or_else(|| "traefik".into()),
        cloudflared_unit: args
            .cloudflared_unit
            .clone()
            .unwrap_or_else(|| base.ingress.cloudflared.unit.clone()),
        frps,
        frps_config,
        frps_unit: args
            .frps_unit
            .clone()
            .unwrap_or_else(|| base.frp.unit.clone()),
        start: tri(args.start, args.no_start).unwrap_or(true),
        hardened: args.hardened,
        log_level: base.log.level.clone(),
    };
    Ok(Resolved { params, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use crate::ops::host::FakeHost;
    use crate::ops::prompt::Scripted;
    use clap::Parser;

    fn parse(extra: &[&str]) -> InstallArgs {
        let mut v = vec!["yanto", "install"];
        v.extend_from_slice(extra);
        match Cli::try_parse_from(v).unwrap().command {
            Command::Install(a) => a,
            _ => unreachable!(),
        }
    }

    fn layout_with_users() -> (tempfile::TempDir, Layout) {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path());
        std::fs::create_dir_all(l.path("/etc")).unwrap();
        std::fs::write(
            l.path(PASSWD),
            "root:x:0:0:root:/root:/bin/bash\ndeploy:x:1000:1000::/home/deploy:/bin/bash\nalice:x:1001:1001::/home/alice:/bin/bash\n",
        )
        .unwrap();
        (d, l)
    }

    fn run_ni(args: &InstallArgs, host: &FakeHost, l: &Layout) -> anyhow::Result<Resolved> {
        resolve(
            args,
            host,
            l,
            Path::new("/etc/yanto/config.toml"),
            None,
            None,
        )
    }

    #[test]
    fn flags_parse_including_aliases_and_pairs() {
        let a = parse(&[
            "--yes",
            "--bind",
            "127.0.0.1",
            "--port",
            "9000",
            "--ingress",
            "traefik,cloudflare-tunnel",
            "--ingress",
            "none",
            "--enable-registry",
            "--no-start",
            "--frps",
            "--no-frps",
        ]);
        assert!(a.non_interactive);
        assert_eq!(a.port, Some(9000));
        assert_eq!(a.ingress.len(), 3);
        assert_eq!(tri(a.enable_registry, a.no_enable_registry), Some(true));
        assert_eq!(tri(a.start, a.no_start), Some(false));
        assert_eq!(tri(a.frps, a.no_frps), Some(false), "last flag wins");
        assert!(Cli::try_parse_from(["yanto", "install", "--ingress", "nginx"]).is_err());
    }

    #[test]
    fn non_interactive_uses_defaults_and_derives_user_from_sudo_user() {
        let (_d, l) = layout_with_users();
        let mut h = FakeHost::default();
        h.env.insert("SUDO_USER".into(), "alice".into());
        let r = run_ni(&parse(&["--non-interactive"]), &h, &l).unwrap();
        assert_eq!(r.params.ssh_user, "alice");
        assert_eq!(r.params.port, 9197);
        assert_eq!(r.params.bind.to_string(), "127.0.0.1");
        assert!(r.params.ingress.is_empty());
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn user_falls_back_to_first_regular_user_and_errors_when_none() {
        let (_d, l) = layout_with_users();
        let r = run_ni(&parse(&["--yes"]), &FakeHost::default(), &l).unwrap();
        assert_eq!(r.params.ssh_user, "deploy");
        std::fs::write(l.path(PASSWD), "root:x:0:0:root:/root:/bin/bash\n").unwrap();
        let e = run_ni(&parse(&["--yes"]), &FakeHost::default(), &l)
            .err()
            .unwrap();
        assert!(e.to_string().contains("--ssh-user"));
    }

    #[test]
    fn explicit_unknown_user_rejected_root_warns() {
        let (_d, l) = layout_with_users();
        let h = FakeHost::default();
        assert!(run_ni(&parse(&["--yes", "--ssh-user", "ghost"]), &h, &l).is_err());
        let r = run_ni(&parse(&["--yes", "--ssh-user", "root"]), &h, &l).unwrap();
        assert!(r.warnings.iter().any(|w| w.contains("root")));
    }

    #[test]
    fn public_bind_needs_opt_in_non_interactively() {
        let (_d, l) = layout_with_users();
        let h = FakeHost::default();
        assert!(run_ni(&parse(&["--yes", "--bind", "0.0.0.0"]), &h, &l).is_err());
        let r = run_ni(
            &parse(&["--yes", "--bind", "0.0.0.0", "--allow-public-bind"]),
            &h,
            &l,
        )
        .unwrap();
        assert!(r.warnings.iter().any(|w| w.contains("not loopback")));
    }

    #[test]
    fn invalid_values_rejected() {
        let (_d, l) = layout_with_users();
        let h = FakeHost::default();
        assert!(run_ni(&parse(&["--yes", "--bind", "localhost"]), &h, &l).is_err());
        assert!(run_ni(&parse(&["--yes", "--data-dir", "rel/dir"]), &h, &l).is_err());
        assert!(run_ni(&parse(&["--yes", "--public-url", "ftp://x"]), &h, &l).is_err());
        assert!(Cli::try_parse_from(["yanto", "install", "--port", "0x"]).is_err());
    }

    #[test]
    fn ingress_none_is_dropped_and_traefik_flags_apply() {
        let (_d, l) = layout_with_users();
        let r = run_ni(
            &parse(&[
                "--yes",
                "--ingress",
                "none,traefik",
                "--traefik-dynamic-dir",
                "/x/dyn",
                "--frps",
            ]),
            &FakeHost::default(),
            &l,
        )
        .unwrap();
        assert_eq!(r.params.ingress, vec![Ingress::Traefik]);
        assert_eq!(r.params.traefik_dynamic_dir, PathBuf::from("/x/dyn"));
        assert!(r.params.frps);
        assert!(r.warnings.iter().any(|w| w.contains("does not exist")));
    }

    #[test]
    fn flags_suppress_their_prompts_and_prompts_fill_the_rest() {
        let (_d, l) = layout_with_users();
        let a = parse(&["--port", "9300", "--ssh-user", "deploy", "--no-frps"]);
        let mut s = Scripted::default();
        s.texts
            .extend(["127.0.0.1".into(), "".into(), "/var/lib/yanto".into()]);
        s.confirms.push_back(true); // registry
        s.multis.push_back(vec![1]); // traefik
        s.texts.extend(["/etc/traefik/dynamic".into(), "le".into()]);
        let r = resolve(
            &a,
            &FakeHost::default(),
            &l,
            Path::new("/c.toml"),
            None,
            Some(&mut s),
        )
        .unwrap();
        assert_eq!(r.params.port, 9300);
        assert!(r.params.registry);
        assert_eq!(r.params.ingress, vec![Ingress::Traefik]);
        assert_eq!(r.params.traefik_cert_resolver, "le");
        assert!(
            !s.asked
                .iter()
                .any(|m| m == "Port" || m.contains("Unix user") || m.contains("frps"))
        );
    }

    #[test]
    fn busy_port_reasks() {
        let (_d, l) = layout_with_users();
        let mut h = FakeHost::default();
        h.busy_ports.insert(9197);
        let a = parse(&[
            "--ssh-user",
            "deploy",
            "--no-enable-registry",
            "--ingress",
            "none",
            "--no-frps",
        ]);
        let mut s = Scripted::default();
        s.texts.extend([
            "127.0.0.1".into(),
            "9197".into(),
            "9198".into(),
            "".into(),
            "/var/lib/yanto".into(),
        ]);
        let r = resolve(&a, &h, &l, Path::new("/c.toml"), None, Some(&mut s)).unwrap();
        assert_eq!(r.params.port, 9198);
    }
}
