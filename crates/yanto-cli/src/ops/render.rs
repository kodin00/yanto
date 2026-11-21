//! Deterministic rendering of `config.toml` and the systemd unit
//! (docs/rewrite/01 section 8.1, 05 section 5).

use std::path::{Path, PathBuf};

use super::layout::BIN_PATH;
use super::params::InstallParams;

fn q(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn qp(p: &Path) -> String {
    q(&p.to_string_lossy())
}

/// Render `/etc/yanto/config.toml`. Never contains secrets.
pub fn render_config(p: &InstallParams) -> String {
    let providers = p
        .ingress
        .iter()
        .filter_map(|i| i.provider_name())
        .map(q)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"# Written by `yanto install`. No secrets live in this file.
# Paths that appear in the systemd unit's ReadWritePaths are only changed via
# `yanto install --reconfigure`, which re-renders the unit.
config_version = 1

[server]
bind = {bind}
port = {port}
public_url = {public_url}

[paths]
data_dir = {data_dir}

[ssh]
authorized_keys_user = {ssh_user}

[docker]
socket = "/var/run/docker.sock"

[registry]
enabled = {registry}

[ingress]
providers = [{providers}]

[ingress.traefik]
dynamic_dir = {traefik_dir}
cert_resolver = {resolver}

[ingress.cloudflared]
unit = {cf_unit}

[frp]
enabled = {frps}
config_path = {frps_config}
unit = {frps_unit}

[deploy]
max_concurrent = 2
command_timeout = "1h"
log_max_bytes = 500000

[log]
level = {level}
format = "json"
"#,
        bind = q(&p.bind.to_string()),
        port = p.port,
        public_url = q(&p.public_url),
        data_dir = qp(&p.data_dir),
        ssh_user = q(&p.ssh_user),
        registry = p.registry,
        traefik_dir = qp(&p.traefik_dynamic_dir),
        resolver = q(&p.traefik_cert_resolver),
        cf_unit = q(&p.cloudflared_unit),
        frps = p.frps,
        frps_config = qp(&p.frps_config),
        frps_unit = q(&p.frps_unit),
        level = q(&p.log_level),
    )
}

/// Paths the unit must list in `ReadWritePaths` (05 section 5, Tier B).
/// Optional host paths are prefixed with `-` so a missing dir does not fail
/// the unit. `ssh_dir` is the configured user's `~/.ssh`.
pub fn read_write_paths(p: &InstallParams, ssh_dir: Option<&Path>) -> Vec<String> {
    let mut v = vec![
        p.data_dir.to_string_lossy().into_owned(),
        "/etc/cron.d".into(),
    ];
    v.push(format!("-{}", p.traefik_dynamic_dir.display()));
    if let Some(dir) = p.frps_config.parent() {
        v.push(format!("-{}", dir.display()));
    }
    if let Some(d) = ssh_dir {
        v.push(format!("-{}", d.display()));
    }
    v.dedup();
    v
}

pub fn ssh_dir_for(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|h| h.join(".ssh"))
}

/// Render `/etc/systemd/system/yanto.service`.
pub fn render_unit(p: &InstallParams, ssh_dir: Option<&Path>) -> String {
    let rw = read_write_paths(p, ssh_dir).join(" ");
    // StateDirectory only covers the default location.
    let state = if p.data_dir == Path::new("/var/lib/yanto") {
        "StateDirectory=yanto\nStateDirectoryMode=0700\n"
    } else {
        ""
    };
    let hardened = if p.hardened {
        "\n# --- optional --hardened profile ---\nCapabilityBoundingSet=CAP_CHOWN CAP_DAC_OVERRIDE CAP_FOWNER CAP_KILL CAP_SETUID CAP_SETGID\nAmbientCapabilities=CAP_CHOWN CAP_DAC_OVERRIDE CAP_FOWNER CAP_KILL CAP_SETUID CAP_SETGID\n"
    } else {
        ""
    };
    format!(
        "[Unit]
Description=Yanto VPS management
Documentation=https://github.com/kodin00/yanto
After=network-online.target docker.service
Wants=network-online.target docker.service

[Service]
Type=simple
User=root
ExecStart={BIN_PATH} serve --config {config}
Restart=on-failure
RestartSec=2
TimeoutStopSec=45
KillMode=control-group
UMask=0077
LimitNOFILE=65536
Environment=HOME=/root
{state}RuntimeDirectory=yanto
RuntimeDirectoryMode=0700

# --- sandboxing: Tier A (safe, always on) ---
NoNewPrivileges=yes
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
LockPersonality=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
SystemCallArchitectures=native
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK

# --- Tier B (on; paths generated from config) ---
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths={rw}
{hardened}
[Install]
WantedBy=multi-user.target
",
        config = p.config_path.display(),
    )
}

/// Extract `ReadWritePaths=` entries (leading `-` stripped) from a unit file.
pub fn unit_read_write_paths(unit: &str) -> Vec<String> {
    unit.lines()
        .filter_map(|l| l.strip_prefix("ReadWritePaths="))
        .flat_map(str::split_whitespace)
        .map(|s| s.trim_start_matches('-').to_string())
        .collect()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::super::params::{Ingress, InstallParams};

    pub fn params() -> InstallParams {
        InstallParams {
            config_path: "/etc/yanto/config.toml".into(),
            bind: "127.0.0.1".parse().unwrap(),
            port: 9197,
            public_url: String::new(),
            data_dir: "/var/lib/yanto".into(),
            ssh_user: "deploy".into(),
            registry: false,
            ingress: vec![Ingress::CloudflareTunnel, Ingress::Traefik],
            traefik_dynamic_dir: "/etc/traefik/dynamic".into(),
            traefik_cert_resolver: "letsencrypt".into(),
            traefik_unit: "traefik".into(),
            cloudflared_unit: "cloudflared".into(),
            frps: false,
            frps_config: "/etc/frp/frps.toml".into(),
            frps_unit: "frps".into(),
            start: true,
            hardened: false,
            log_level: "info".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::params;
    use super::*;
    use yanto_core::Config;

    #[test]
    fn default_config_roundtrips_to_core_defaults() {
        let text = render_config(&params());
        let cfg = Config::from_toml_str(&text, Path::new("t")).unwrap();
        assert_eq!(cfg, Config::default());
        insta::assert_snapshot!(text);
    }

    #[test]
    fn custom_values_roundtrip_and_escape() {
        let mut p = params();
        p.port = 8080;
        p.bind = "10.0.0.5".parse().unwrap();
        p.public_url = "https://y.example.com".into();
        p.data_dir = "/srv/\"y\"".into();
        p.registry = true;
        p.frps = true;
        p.ingress = vec![];
        let cfg = Config::from_toml_str(&render_config(&p), Path::new("t")).unwrap();
        assert_eq!(cfg.server.port, 8080);
        assert_eq!(cfg.server.bind, "10.0.0.5");
        assert_eq!(cfg.server.public_url, "https://y.example.com");
        assert_eq!(cfg.paths.data_dir, PathBuf::from("/srv/\"y\""));
        assert!(cfg.registry.enabled && cfg.frp.enabled);
        assert!(cfg.ingress.providers.is_empty());
    }

    #[test]
    fn unit_snapshot_default() {
        insta::assert_snapshot!(render_unit(&params(), Some(Path::new("/home/deploy/.ssh"))));
    }

    #[test]
    fn unit_custom_data_dir_drops_state_directory_and_hardened_adds_caps() {
        let mut p = params();
        p.data_dir = "/srv/yanto".into();
        p.hardened = true;
        let u = render_unit(&p, None);
        assert!(!u.contains("StateDirectory"));
        assert!(u.contains("CapabilityBoundingSet="));
        assert!(u.contains("ReadWritePaths=/srv/yanto /etc/cron.d"));
        assert!(!u.contains(".ssh"));
    }

    #[test]
    fn must_not_set_list_is_absent() {
        let u = render_unit(&params(), None);
        for bad in [
            "ProcSubset",
            "ProtectProc",
            "PrivateUsers",
            "PrivateNetwork",
            "PrivateDevices",
            "MemoryDenyWriteExecute",
            "ProtectHome=yes",
            "RestrictNamespaces",
            "SystemCallFilter",
        ] {
            assert!(!u.contains(bad), "{bad}");
        }
    }

    #[test]
    fn rw_paths_parse_back() {
        let u = render_unit(&params(), Some(Path::new("/home/deploy/.ssh")));
        let got = unit_read_write_paths(&u);
        assert_eq!(
            got,
            vec![
                "/var/lib/yanto",
                "/etc/cron.d",
                "/etc/traefik/dynamic",
                "/etc/frp",
                "/home/deploy/.ssh"
            ]
        );
    }
}
