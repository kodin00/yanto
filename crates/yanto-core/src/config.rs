//! Static bootstrap configuration (`/etc/yanto/config.toml`, docs/rewrite/01 section 8.1).
//!
//! The file never holds secrets. Operator-editable runtime settings live in the
//! database, not here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/yanto/config.toml";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config file {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid config: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub config_version: u32,
    pub server: ServerConfig,
    pub paths: PathsConfig,
    pub ssh: SshConfig,
    pub docker: DockerConfig,
    pub registry: RegistryConfig,
    pub ingress: IngressConfig,
    pub frp: FrpConfig,
    pub deploy: DeployConfig,
    pub log: LogConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
    /// Optional; drives cookie `Secure` and the CSRF origin check.
    pub public_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathsConfig {
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SshConfig {
    pub authorized_keys_user: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DockerConfig {
    pub socket: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct RegistryConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IngressConfig {
    pub providers: Vec<String>,
    pub traefik: TraefikConfig,
    pub cloudflared: CloudflaredConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TraefikConfig {
    pub dynamic_dir: PathBuf,
    pub cert_resolver: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CloudflaredConfig {
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FrpConfig {
    pub enabled: bool,
    pub config_path: PathBuf,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeployConfig {
    pub max_concurrent: u32,
    /// Duration string such as `30s`, `15m`, `1h`.
    pub command_timeout: String,
    pub log_max_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Json,
    Pretty,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    pub level: String,
    pub format: LogFormat,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: 1,
            server: ServerConfig::default(),
            paths: PathsConfig::default(),
            ssh: SshConfig::default(),
            docker: DockerConfig::default(),
            registry: RegistryConfig::default(),
            ingress: IngressConfig::default(),
            frp: FrpConfig::default(),
            deploy: DeployConfig::default(),
            log: LogConfig::default(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".into(),
            port: 9197,
            public_url: String::new(),
        }
    }
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            data_dir: "/var/lib/yanto".into(),
        }
    }
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            authorized_keys_user: "deploy".into(),
        }
    }
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            socket: "/var/run/docker.sock".into(),
        }
    }
}

impl Default for IngressConfig {
    fn default() -> Self {
        Self {
            providers: vec!["cloudflare-tunnel".into(), "traefik".into()],
            traefik: TraefikConfig::default(),
            cloudflared: CloudflaredConfig::default(),
        }
    }
}

impl Default for TraefikConfig {
    fn default() -> Self {
        Self {
            dynamic_dir: "/etc/traefik/dynamic".into(),
            cert_resolver: "letsencrypt".into(),
        }
    }
}

impl Default for CloudflaredConfig {
    fn default() -> Self {
        Self {
            unit: "cloudflared".into(),
        }
    }
}

impl Default for FrpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            config_path: "/etc/frp/frps.toml".into(),
            unit: "frps".into(),
        }
    }
}

impl Default for DeployConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 2,
            command_timeout: "1h".into(),
            log_max_bytes: 500_000,
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: LogFormat::Json,
        }
    }
}

impl Config {
    /// Parse a TOML document. Missing keys take their defaults.
    pub fn from_toml_str(text: &str, origin: &Path) -> Result<Self, ConfigError> {
        let cfg: Config = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: origin.to_path_buf(),
            source,
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Read and parse a config file; the file must exist.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&text, path)
    }

    /// Apply the documented environment overrides:
    /// `YANTO_SERVER__BIND`, `YANTO_SERVER__PORT`, `YANTO_LOG__LEVEL`.
    pub fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) -> Result<(), ConfigError> {
        if let Some(v) = get("YANTO_SERVER__BIND") {
            self.server.bind = v;
        }
        if let Some(v) = get("YANTO_SERVER__PORT") {
            self.server.port = v.parse().map_err(|_| {
                ConfigError::Invalid(format!("YANTO_SERVER__PORT is not a port: {v:?}"))
            })?;
        }
        if let Some(v) = get("YANTO_LOG__LEVEL") {
            self.log.level = v;
        }
        self.validate()
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.config_version != 1 {
            return Err(ConfigError::Invalid(format!(
                "unsupported config_version {} (expected 1)",
                self.config_version
            )));
        }
        if self.server.port == 0 {
            return Err(ConfigError::Invalid("server.port must be 1-65535".into()));
        }
        if self.deploy.max_concurrent == 0 {
            return Err(ConfigError::Invalid(
                "deploy.max_concurrent must be at least 1".into(),
            ));
        }
        self.command_timeout()?;
        Ok(())
    }

    /// `deploy.command_timeout` as a [`Duration`].
    pub fn command_timeout(&self) -> Result<Duration, ConfigError> {
        parse_duration(&self.deploy.command_timeout)
    }

    /// Path of the SQLite database inside the data dir.
    pub fn db_path(&self) -> PathBuf {
        self.paths.data_dir.join("yanto.db")
    }
}

/// Parses `<int><unit>` with unit `s`, `m` or `h`.
fn parse_duration(s: &str) -> Result<Duration, ConfigError> {
    let bad = || {
        ConfigError::Invalid(format!(
            "deploy.command_timeout {s:?} is not a duration like 30s, 15m, 1h"
        ))
    };
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u64 = num.parse().map_err(|_| bad())?;
    let secs = match unit {
        "s" => n,
        "m" => n.checked_mul(60).ok_or_else(bad)?,
        "h" => n.checked_mul(3600).ok_or_else(bad)?,
        _ => return Err(bad()),
    };
    if secs == 0 {
        return Err(bad());
    }
    Ok(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::from_toml_str(text, Path::new("test.toml"))
    }

    #[test]
    fn empty_file_gives_documented_defaults() {
        let cfg = parse("").unwrap();
        assert_eq!(cfg.server.bind, "127.0.0.1");
        assert_eq!(cfg.server.port, 9197);
        assert_eq!(cfg.paths.data_dir, PathBuf::from("/var/lib/yanto"));
        assert_eq!(cfg.log.format, LogFormat::Json);
        assert_eq!(cfg.command_timeout().unwrap(), Duration::from_secs(3600));
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn full_example_from_architecture_doc_parses() {
        let cfg = parse(
            r#"
config_version = 1
[server]
bind = "127.0.0.1"
port = 9197
public_url = ""
[paths]
data_dir = "/var/lib/yanto"
[ssh]
authorized_keys_user = "deploy"
[docker]
socket = "/var/run/docker.sock"
[registry]
enabled = false
[ingress]
providers = ["cloudflare-tunnel", "traefik"]
[ingress.traefik]
dynamic_dir = "/etc/traefik/dynamic"
cert_resolver = "letsencrypt"
[ingress.cloudflared]
unit = "cloudflared"
[frp]
enabled = false
config_path = "/etc/frp/frps.toml"
unit = "frps"
[deploy]
max_concurrent = 2
command_timeout = "1h"
log_max_bytes = 500000
[log]
level = "info"
format = "json"
"#,
        )
        .unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn overrides_and_pretty_format() {
        let cfg = parse("[server]\nport = 8080\n[log]\nformat = \"pretty\"\nlevel = \"debug\"\n")
            .unwrap();
        assert_eq!(cfg.server.port, 8080);
        assert_eq!(cfg.server.bind, "127.0.0.1");
        assert_eq!(cfg.log.format, LogFormat::Pretty);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(matches!(
            parse("[server]\nprot = 1\n"),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn invalid_values_are_rejected() {
        assert!(parse("config_version = 2").is_err());
        assert!(parse("[server]\nport = 0").is_err());
        assert!(parse("[deploy]\ncommand_timeout = \"soon\"").is_err());
        assert!(parse("[deploy]\nmax_concurrent = 0").is_err());
    }

    #[test]
    fn env_overrides_apply() {
        let mut cfg = Config::default();
        cfg.apply_env(|k| match k {
            "YANTO_SERVER__PORT" => Some("9000".into()),
            "YANTO_LOG__LEVEL" => Some("debug".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cfg.server.port, 9000);
        assert_eq!(cfg.log.level, "debug");
        let err = cfg.apply_env(|k| (k == "YANTO_SERVER__PORT").then(|| "nope".to_string()));
        assert!(err.is_err());
    }

    #[test]
    fn load_reports_missing_file() {
        let err = Config::load(Path::new("/nonexistent/yanto.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }));
    }

    #[test]
    fn load_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.toml");
        std::fs::write(&path, "[paths]\ndata_dir = \"/tmp/x\"\n").unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.db_path(), PathBuf::from("/tmp/x/yanto.db"));
    }
}
