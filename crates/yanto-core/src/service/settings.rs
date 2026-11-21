//! Runtime settings (docs/rewrite/02 4.5, 03 `settings_read` / `settings_update`).
//!
//! Operator-tunable, non-secret values live in `app_settings`. They override the matching
//! `config.toml` value and are read once at startup, so a change takes effect after the next
//! restart of `yanto serve`. Host paths and integration units stay in `config.toml` and are shown
//! read-only. Owner only.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::Serialize;
use serde_json::{Value, json};

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use crate::actor::{Access, Actor, OpSpec, Scope};
use crate::config::{Config, DeployConfig};
use crate::db::Db;
use crate::db::repo::{db_err, settings};
use crate::error::CoreError;

pub const OP_SETTINGS_READ: OpSpec = OpSpec::new("settings_read", Access::Owner, Scope::Read);
pub const OP_SETTINGS_UPDATE: OpSpec = OpSpec::new("settings_update", Access::Owner, Scope::Admin);

pub const OPERATIONS: &[&OpSpec] = &[&OP_SETTINGS_READ, &OP_SETTINGS_UPDATE];

pub const KEY_MAX_CONCURRENT: &str = "deploy.max_concurrent";
pub const KEY_RETENTION: &str = "deploy.retention_per_project";
pub const KEY_COMMAND_TIMEOUT_MS: &str = "runtime.command_timeout_ms";
pub const KEY_LOG_MAX_BYTES: &str = "runtime.deployment_log_max_bytes";
pub const KEY_TRUSTED_PROXIES: &str = "runtime.trusted_proxies";

#[derive(Clone, Copy)]
enum Kind {
    Int { min: i64, max: i64 },
    Cidrs,
}

struct Def {
    key: &'static str,
    label: &'static str,
    help: &'static str,
    unit: &'static str,
    kind: Kind,
    default: fn() -> Value,
}

fn defaults() -> DeployConfig {
    DeployConfig::default()
}

const DEFS: &[Def] = &[
    Def {
        key: KEY_MAX_CONCURRENT,
        label: "Concurrent Deployments",
        help: "How many Deployments run at the same time (one per Project).",
        unit: "",
        kind: Kind::Int { min: 1, max: 16 },
        default: || json!(defaults().max_concurrent),
    },
    Def {
        key: KEY_RETENTION,
        label: "Deployments kept per Project",
        help: "Older finished Deployments and their logs are deleted.",
        unit: "",
        kind: Kind::Int { min: 1, max: 1000 },
        default: || json!(defaults().retention_per_project),
    },
    Def {
        key: KEY_COMMAND_TIMEOUT_MS,
        label: "Command timeout",
        help: "Longest a single git or docker command of a Deployment may run.",
        unit: "ms",
        kind: Kind::Int {
            min: 1_000,
            max: 86_400_000,
        },
        default: || json!(3_600_000),
    },
    Def {
        key: KEY_LOG_MAX_BYTES,
        label: "Deployment log size",
        help: "Stored log per Deployment; the oldest output is dropped first.",
        unit: "bytes",
        kind: Kind::Int {
            min: 16_384,
            max: 100_000_000,
        },
        default: || json!(defaults().log_max_bytes),
    },
    Def {
        key: KEY_TRUSTED_PROXIES,
        label: "Trusted proxies",
        help: "Peers (CIDR) whose X-Forwarded-For / CF-Connecting-IP headers are believed.",
        unit: "",
        kind: Kind::Cidrs,
        default: || json!(["127.0.0.1/32", "::1/128"]),
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct SettingEntry {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub unit: &'static str,
    /// `int` or `cidr_list`.
    pub kind: &'static str,
    pub min: Option<i64>,
    pub max: Option<i64>,
    /// Stored value, or the built-in default when none is stored.
    pub value: Value,
    pub default: Value,
    pub overridden: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostEntry {
    pub label: &'static str,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub runtime: Vec<SettingEntry>,
    /// `config.toml` values, read-only here.
    pub host: Vec<HostEntry>,
    /// Runtime settings are read at startup.
    pub applies: &'static str,
}

/// Patch: key to new value; `null` removes the stored value (back to the default).
pub type SettingsPatch = BTreeMap<String, Value>;

#[derive(Debug, Clone)]
pub struct SettingsService {
    ctx: Ctx,
    host: Vec<HostEntry>,
}

fn def(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}

/// Normalize and validate one value for its definition.
fn check(d: &Def, v: &Value) -> Result<Value, CoreError> {
    match d.kind {
        Kind::Int { min, max } => {
            let n = v.as_i64().ok_or_else(|| {
                CoreError::validation(d.key, format!("{} must be a whole number", d.label))
            })?;
            if !(min..=max).contains(&n) {
                return Err(CoreError::validation(
                    d.key,
                    format!("{} must be between {min} and {max}", d.label),
                ));
            }
            Ok(json!(n))
        }
        Kind::Cidrs => {
            let items: Vec<&str> = match v {
                Value::Array(a) => a
                    .iter()
                    .map(|x| x.as_str())
                    .collect::<Option<_>>()
                    .ok_or_else(|| {
                        CoreError::validation(d.key, "trusted proxies must be a list of strings")
                    })?,
                _ => {
                    return Err(CoreError::validation(
                        d.key,
                        "trusted proxies must be a list of strings",
                    ));
                }
            };
            let mut out = Vec::new();
            for raw in items {
                let s = raw.trim();
                if s.is_empty() {
                    continue;
                }
                if !valid_cidr(s) {
                    return Err(CoreError::validation(
                        d.key,
                        format!("{s:?} is not an IP address or CIDR such as 10.0.0.0/8"),
                    ));
                }
                out.push(s.to_owned());
            }
            if out.len() > 64 {
                return Err(CoreError::validation(d.key, "at most 64 entries"));
            }
            Ok(json!(out))
        }
    }
}

fn valid_cidr(s: &str) -> bool {
    let (a, p) = s.split_once('/').unwrap_or((s, ""));
    let Ok(addr) = a.parse::<IpAddr>() else {
        return false;
    };
    if p.is_empty() {
        return true;
    }
    let max = if addr.is_ipv4() { 32 } else { 128 };
    p.parse::<u8>().is_ok_and(|n| n <= max)
}

impl SettingsService {
    pub fn new(ctx: Ctx, config: &Config) -> Self {
        let h = |label, value: String| HostEntry { label, value };
        let host = vec![
            h("Bind address", config.server.bind.clone()),
            h("Port", config.server.port.to_string()),
            h(
                "Public URL",
                if config.server.public_url.is_empty() {
                    "(not set)".into()
                } else {
                    config.server.public_url.clone()
                },
            ),
            h(
                "Data directory",
                config.paths.data_dir.display().to_string(),
            ),
            h(
                "Authorized Keys user",
                config.ssh.authorized_keys_user.clone(),
            ),
            h(
                "Traefik dynamic directory",
                config.ingress.traefik.dynamic_dir.display().to_string(),
            ),
            h("frps config", config.frp.config_path.display().to_string()),
        ];
        Self { ctx, host }
    }

    pub async fn read(&self, actor: &Actor) -> Result<SettingsView, CoreError> {
        actor.authorize(&OP_SETTINGS_READ, None)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let mut runtime = Vec::new();
        for d in DEFS {
            let stored = settings::get(&mut conn, d.key)
                .await?
                .and_then(|s| serde_json::from_str::<Value>(&s).ok());
            let default = (d.default)();
            let (kind, min, max) = match d.kind {
                Kind::Int { min, max } => ("int", Some(min), Some(max)),
                Kind::Cidrs => ("cidr_list", None, None),
            };
            runtime.push(SettingEntry {
                key: d.key,
                label: d.label,
                help: d.help,
                unit: d.unit,
                kind,
                min,
                max,
                overridden: stored.is_some(),
                value: stored.unwrap_or_else(|| default.clone()),
                default,
            });
        }
        Ok(SettingsView {
            runtime,
            host: self.host.clone(),
            applies: "Runtime settings are read when Yanto starts; restart the service to apply a change.",
        })
    }

    /// Apply a patch atomically: every entry is validated first. Returns the new view.
    pub async fn update(
        &self,
        actor: &Actor,
        patch: &SettingsPatch,
    ) -> Result<SettingsView, CoreError> {
        actor.authorize(&OP_SETTINGS_UPDATE, None)?;
        if patch.is_empty() {
            return Err(CoreError::validation("settings", "no settings given"));
        }
        let mut changes: Vec<(&'static str, Option<Value>)> = Vec::new();
        for (key, v) in patch {
            let d =
                def(key).ok_or_else(|| CoreError::validation(key.as_str(), "unknown setting"))?;
            changes.push((
                d.key,
                if v.is_null() {
                    None
                } else {
                    Some(check(d, v)?)
                },
            ));
        }
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        for (key, v) in &changes {
            match v {
                Some(v) => settings::set(&mut tx, key, &v.to_string(), now).await?,
                None => settings::delete(&mut tx, key).await?,
            }
        }
        let keys: Vec<&str> = changes.iter().map(|(k, _)| *k).collect();
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("settings.update", "settings")
                .meta(json!({ "keys": keys, "values": changes.iter().map(|(k, v)| json!({ *k: v })).collect::<Vec<_>>() })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        self.read(actor).await
    }
}

/// Overlay stored runtime settings on the file configuration. Called once at startup, before the
/// services are built. Invalid stored values are ignored (the file value stays).
pub async fn apply_overrides(db: &Db, config: &mut Config) {
    let Ok(mut conn) = db.reader().acquire().await else {
        return;
    };
    let get = async |conn: &mut sqlx::SqliteConnection, key: &str| -> Option<i64> {
        settings::get(conn, key)
            .await
            .ok()??
            .parse::<i64>()
            .ok()
            .filter(|n| def(key).is_some_and(|d| check(d, &json!(n)).is_ok()))
    };
    if let Some(n) = get(&mut conn, KEY_MAX_CONCURRENT).await {
        config.deploy.max_concurrent = n as u32;
    }
    if let Some(n) = get(&mut conn, KEY_RETENTION).await {
        config.deploy.retention_per_project = n as u32;
    }
    if let Some(n) = get(&mut conn, KEY_COMMAND_TIMEOUT_MS).await {
        // The file format has second resolution.
        config.deploy.command_timeout = format!("{}s", (n / 1000).max(1));
    }
    if let Some(n) = get(&mut conn, KEY_LOG_MAX_BYTES).await {
        config.deploy.log_max_bytes = n as u64;
    }
}
