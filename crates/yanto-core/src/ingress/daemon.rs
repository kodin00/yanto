//! Host Daemons (ADR 0006, D15): `cloudflared`, `traefik` and `frps` are installed by the operator.
//! Yanto only checks them and shows install instructions; it never installs or auto-runs anything.

use serde::Serialize;

use crate::error::CoreError;
use crate::ports::UnitProps;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Daemon {
    Cloudflared,
    Traefik,
    Frps,
}

impl Daemon {
    pub const ALL: [Daemon; 3] = [Daemon::Cloudflared, Daemon::Traefik, Daemon::Frps];

    pub fn as_str(self) -> &'static str {
        match self {
            Daemon::Cloudflared => "cloudflared",
            Daemon::Traefik => "traefik",
            Daemon::Frps => "frps",
        }
    }

    pub fn parse(s: &str) -> Result<Self, CoreError> {
        Daemon::ALL
            .into_iter()
            .find(|d| d.as_str() == s)
            .ok_or_else(|| {
                CoreError::validation("name", "daemon must be cloudflared, traefik or frps")
            })
    }

    /// Program and arguments that print the version (PATH lookup for cloudflared and Traefik).
    pub fn version_args(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Daemon::Cloudflared => ("cloudflared", &["--version"]),
            Daemon::Traefik => ("traefik", &["version"]),
            Daemon::Frps => ("frps", &["--version"]),
        }
    }
}

/// Systemd unit names must match `^[A-Za-z0-9@._-]+$` before use (04 8.7).
pub fn valid_unit_name(unit: &str) -> bool {
    !unit.is_empty()
        && unit.len() <= 128
        && !unit.starts_with('-')
        && unit
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'@' | b'.' | b'_' | b'-'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonState {
    NotInstalled,
    Inactive,
    Running,
    Failed,
}

impl DaemonState {
    pub fn from_props(p: &UnitProps) -> Self {
        if p.load_state == "not-found" || p.load_state.is_empty() {
            return DaemonState::NotInstalled;
        }
        match p.active_state.as_str() {
            "active" | "activating" | "reloading" => DaemonState::Running,
            "failed" => DaemonState::Failed,
            _ => DaemonState::Inactive,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DaemonState::NotInstalled => "not installed",
            DaemonState::Inactive => "installed, inactive",
            DaemonState::Running => "running",
            DaemonState::Failed => "failed",
        }
    }
}

/// Parse `systemctl show` output (`Key=Value` lines).
pub fn parse_show(out: &str) -> UnitProps {
    let mut p = UnitProps::default();
    for line in out.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k {
            "LoadState" => p.load_state = v.trim().to_owned(),
            "ActiveState" => p.active_state = v.trim().to_owned(),
            "SubState" => p.sub_state = v.trim().to_owned(),
            "FragmentPath" if !v.trim().is_empty() => p.fragment_path = Some(v.trim().to_owned()),
            "MainPID" => p.main_pid = v.trim().parse().ok().filter(|n| *n != 0),
            "ExecStart" => p.exec_start = v.trim().to_owned(),
            _ => {}
        }
    }
    p
}

/// `(binary, argv)` from systemd's `ExecStart={ path=... ; argv[]=... ; ... }`.
pub fn parse_exec_start(raw: &str) -> Option<(String, Vec<String>)> {
    let field = |key: &str| -> Option<&str> {
        let start = raw.find(key)? + key.len();
        let rest = &raw[start..];
        let end = rest
            .find(" ;")
            .or_else(|| rest.find(" }"))
            .unwrap_or(rest.len());
        Some(rest[..end].trim())
    };
    let path = field("path=")?;
    if path.is_empty() {
        return None;
    }
    let argv = field("argv[]=")
        .map(|a| a.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    Some((path.to_owned(), argv))
}

/// `-c <path>`, `--config <path>`, `--config=<path>` or `-c=<path>` from an argument list.
pub fn config_path_from_args(argv: &[String]) -> Option<String> {
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        if a == "-c" || a == "--config" || a == "-config" {
            return it.next().cloned();
        }
        for p in ["--config=", "-c=", "-config="] {
            if let Some(v) = a.strip_prefix(p) {
                return Some(v.to_owned());
            }
        }
    }
    None
}

/// Sample `frps` unit shown to the operator; Yanto never writes it.
pub const FRPS_SAMPLE_UNIT: &str = "[Unit]
Description=frp server
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/frps -c /etc/frp/frps.toml
Restart=on-failure

[Install]
WantedBy=multi-user.target
";

/// Operator instructions for a daemon that is not installed. Yanto shows them and does nothing.
pub fn install_instructions(d: Daemon, unit: &str) -> Vec<String> {
    match d {
        Daemon::Cloudflared => vec![
            "Install cloudflared from Cloudflare's official downloads (package or binary) as root.".into(),
            "Create a tunnel on the Cloudflare page in Yanto, then reveal its install command.".into(),
            format!("Run the revealed `cloudflared service install <TOKEN>` as root; it creates and starts the `{unit}` service."),
            "Yanto never runs this command and never stores the token.".into(),
        ],
        Daemon::Traefik => vec![
            "Install Traefik from its official releases or your package manager as root.".into(),
            format!("Create a systemd unit named `{unit}` that starts Traefik with your static configuration."),
            "The static configuration needs entry points, a certificate resolver and a file provider with `watch: true`; the Hostnames page lists the exact prerequisites.".into(),
        ],
        Daemon::Frps => vec![
            "Download frp from https://github.com/fatedier/frp/releases and install the `frps` binary as root.".into(),
            format!("Create the `{unit}` systemd unit, for example the sample below, and `systemctl enable --now {unit}`."),
            "Yanto never writes the unit and never installs frp.".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW: &str = "LoadState=loaded\nActiveState=active\nSubState=running\nFragmentPath=/etc/systemd/system/frps.service\nMainPID=812\nExecStart={ path=/usr/local/bin/frps ; argv[]=/usr/local/bin/frps -c /etc/frp/frps.toml ; ignore_errors=no ; start_time=[n/a] }\n";

    #[test]
    fn show_output_parses() {
        let p = parse_show(SHOW);
        assert_eq!(p.load_state, "loaded");
        assert_eq!(p.main_pid, Some(812));
        assert_eq!(DaemonState::from_props(&p), DaemonState::Running);
        let (bin, argv) = parse_exec_start(&p.exec_start).unwrap();
        assert_eq!(bin, "/usr/local/bin/frps");
        assert_eq!(
            config_path_from_args(&argv).as_deref(),
            Some("/etc/frp/frps.toml")
        );
    }

    #[test]
    fn states() {
        let nf = parse_show(
            "LoadState=not-found\nActiveState=inactive\nSubState=dead\nMainPID=0\nExecStart=\n",
        );
        assert_eq!(DaemonState::from_props(&nf), DaemonState::NotInstalled);
        assert_eq!(nf.main_pid, None);
        let inactive = parse_show("LoadState=loaded\nActiveState=inactive\n");
        assert_eq!(DaemonState::from_props(&inactive), DaemonState::Inactive);
        let failed = parse_show("LoadState=loaded\nActiveState=failed\n");
        assert_eq!(DaemonState::from_props(&failed), DaemonState::Failed);
    }

    #[test]
    fn config_flag_spellings() {
        let a = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            config_path_from_args(&a(&["frps", "--config=/x.toml"])).as_deref(),
            Some("/x.toml")
        );
        assert_eq!(
            config_path_from_args(&a(&["frps", "--config", "/y.toml"])).as_deref(),
            Some("/y.toml")
        );
        assert_eq!(config_path_from_args(&a(&["frps"])), None);
        assert!(parse_exec_start("").is_none());
    }

    #[test]
    fn unit_names() {
        assert!(valid_unit_name("frps"));
        assert!(valid_unit_name("cloudflared@1.service"));
        for bad in ["", "-x", "a b", "a;b", "a/b", "a`b"] {
            assert!(!valid_unit_name(bad));
        }
        assert!(Daemon::parse("nginx").is_err());
    }

    #[test]
    fn instructions_never_contain_a_token() {
        for d in Daemon::ALL {
            for l in install_instructions(d, "x") {
                assert!(!l.contains("eyJ"));
            }
        }
    }
}
