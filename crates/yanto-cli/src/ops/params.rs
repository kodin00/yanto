//! Resolved installer parameters plus `/etc/passwd` helpers.

use std::net::IpAddr;
use std::path::PathBuf;

use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Ingress {
    #[value(name = "cloudflare-tunnel")]
    CloudflareTunnel,
    #[value(name = "traefik")]
    Traefik,
    #[value(name = "none")]
    None,
}

impl Ingress {
    /// Config-file provider name; `None` for the `none` pseudo-value.
    pub fn provider_name(self) -> Option<&'static str> {
        match self {
            Self::CloudflareTunnel => Some("cloudflare-tunnel"),
            Self::Traefik => Some("traefik"),
            Self::None => Option::None,
        }
    }
}

/// Everything the installer needs, after flags/prompts/defaults are merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallParams {
    pub config_path: PathBuf,
    pub bind: IpAddr,
    pub port: u16,
    pub public_url: String,
    pub data_dir: PathBuf,
    pub ssh_user: String,
    pub registry: bool,
    /// Concrete providers only (never contains `None`).
    pub ingress: Vec<Ingress>,
    pub traefik_dynamic_dir: PathBuf,
    pub traefik_cert_resolver: String,
    pub traefik_unit: String,
    pub cloudflared_unit: String,
    pub frps: bool,
    pub frps_config: PathBuf,
    pub frps_unit: String,
    pub start: bool,
    pub hardened: bool,
    pub log_level: String,
}

impl InstallParams {
    /// Rebuild params from a loaded config (upgrade-only and `doctor`).
    pub fn from_config(
        cfg: &yanto_core::Config,
        config_path: &std::path::Path,
    ) -> anyhow::Result<Self> {
        let bind: IpAddr = cfg.server.bind.parse().map_err(|_| {
            anyhow::anyhow!("server.bind {:?} is not an IP address", cfg.server.bind)
        })?;
        let ingress = cfg
            .ingress
            .providers
            .iter()
            .filter_map(|n| match n.as_str() {
                "cloudflare-tunnel" => Some(Ingress::CloudflareTunnel),
                "traefik" => Some(Ingress::Traefik),
                _ => None,
            })
            .collect();
        Ok(Self {
            config_path: config_path.to_path_buf(),
            bind,
            port: cfg.server.port,
            public_url: cfg.server.public_url.clone(),
            data_dir: cfg.paths.data_dir.clone(),
            ssh_user: cfg.ssh.authorized_keys_user.clone(),
            registry: cfg.registry.enabled,
            ingress,
            traefik_dynamic_dir: cfg.ingress.traefik.dynamic_dir.clone(),
            traefik_cert_resolver: cfg.ingress.traefik.cert_resolver.clone(),
            traefik_unit: "traefik".into(),
            cloudflared_unit: cfg.ingress.cloudflared.unit.clone(),
            frps: cfg.frp.enabled,
            frps_config: cfg.frp.config_path.clone(),
            frps_unit: cfg.frp.unit.clone(),
            start: true,
            hardened: false,
            log_level: cfg.log.level.clone(),
        })
    }

    pub fn traefik_selected(&self) -> bool {
        self.ingress.contains(&Ingress::Traefik)
    }

    pub fn cloudflared_selected(&self) -> bool {
        self.ingress.contains(&Ingress::CloudflareTunnel)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdEntry {
    pub name: String,
    pub uid: u32,
    pub home: PathBuf,
}

/// Parse `/etc/passwd` content, skipping malformed lines.
pub fn parse_passwd(text: &str) -> Vec<PasswdEntry> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 6 {
                return None;
            }
            Some(PasswdEntry {
                name: f[0].to_string(),
                uid: f[2].parse().ok()?,
                home: PathBuf::from(f[5]),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwd_parsing_skips_garbage() {
        let t = "root:x:0:0:root:/root:/bin/bash\n# c\nbad line\ndeploy:x:1000:1000::/home/deploy:/bin/bash\n";
        let e = parse_passwd(t);
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].name, "deploy");
        assert_eq!(e[1].home, PathBuf::from("/home/deploy"));
    }

    #[test]
    fn none_has_no_provider_name() {
        assert_eq!(Ingress::None.provider_name(), None);
        assert_eq!(Ingress::Traefik.provider_name(), Some("traefik"));
    }
}
