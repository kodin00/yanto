//! FRP, server side only (docs/rewrite/04 section 8, D15, D17): `frps` status, read/write of the
//! `frps.toml` fields Yanto owns, and the live proxy list from the frps dashboard API.
//!
//! Applying a saved config is a restart of `frps`, done through `daemon_control` (which needs the
//! explicit confirmation of D28). Saving only writes the file and marks "pending restart".

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Value, json};

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use super::daemons::{DaemonDto, DaemonInspector, FRPS_PENDING_KEY, IngressSettings};
use crate::actor::{Access, Actor, OpSpec, Scope};
use crate::db::repo::ingress as repo;
use crate::db::repo::{db_err, settings};
use crate::error::{CoreError, FieldError};
use crate::ingress::daemon::{Daemon, DaemonState};
use crate::ingress::frps::{self, Change, Owned, Patch, PortRange};
use crate::ports::{DashboardTarget, HostProbePort, IngressPorts};
use crate::secrets::Secret;

pub const OP_FRPS_OVERVIEW: OpSpec = OpSpec::new("frps_overview", Access::Owner, Scope::Read);
pub const OP_FRPS_SETTINGS_UPDATE: OpSpec =
    OpSpec::new("frps_settings_update", Access::Owner, Scope::Write);
pub const OP_FRPS_CONFIG_RESTORE: OpSpec =
    OpSpec::new("frps_config_restore", Access::Owner, Scope::Write);
/// Shows `auth.token`, which `frpc` clients need. Dashboard/CLI only and audited.
pub const OP_FRPS_TOKEN_REVEAL: OpSpec =
    OpSpec::new("frps_token_reveal", Access::Owner, Scope::Admin).session_only();

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_FRPS_OVERVIEW,
    &OP_FRPS_SETTINGS_UPDATE,
    &OP_FRPS_CONFIG_RESTORE,
    &OP_FRPS_TOKEN_REVEAL,
];

/// Proxy types listed (04 8.5). `stcp`, `xtcp`, `sudp` are unverified and not queried.
pub const PROXY_TYPES: [&str; 4] = ["tcp", "udp", "http", "https"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrpStatus {
    NotInstalled,
    InstalledInactive,
    Running,
    RunningNoDashboard,
    DashboardUnreachable,
    DashboardAuthFailed,
}

impl FrpStatus {
    pub fn label(self) -> &'static str {
        match self {
            FrpStatus::NotInstalled => "not installed",
            FrpStatus::InstalledInactive => "installed, inactive",
            FrpStatus::Running => "running",
            FrpStatus::RunningNoDashboard => "running (no dashboard)",
            FrpStatus::DashboardUnreachable => "running (dashboard unreachable)",
            FrpStatus::DashboardAuthFailed => "running (dashboard credentials do not match)",
        }
    }
}

/// The owned fields as the UI sees them: secrets appear only as "is set".
#[derive(Debug, Clone, Serialize)]
pub struct FrpConfigView {
    pub path: String,
    pub bind_addr: Option<String>,
    pub bind_port: Option<u16>,
    pub auth_method: Option<String>,
    pub auth_token_set: bool,
    pub web_addr: Option<String>,
    pub web_port: Option<u16>,
    pub web_user: Option<String>,
    pub web_password_set: bool,
    pub allow_ports: Vec<PortRange>,
    pub dashboard_exposed: bool,
    pub world_readable: bool,
    /// Everything Yanto does not own, masked.
    pub unowned_raw: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyDto {
    pub name: String,
    pub proxy_type: String,
    pub status: String,
    pub remote_port: Option<u64>,
    pub traffic_in: u64,
    pub traffic_out: u64,
    pub connections: u64,
    pub last_start: Option<String>,
    pub last_close: Option<String>,
    pub client: Option<String>,
    /// The entry as frps returned it: field names are unverified (OQ-23).
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrpOverview {
    pub status: FrpStatus,
    pub status_label: &'static str,
    pub daemon: DaemonDto,
    pub version: Option<String>,
    pub config: Option<FrpConfigView>,
    pub config_error: Option<String>,
    pub pending_restart: bool,
    pub backup_available: bool,
    pub public_host: String,
    pub proxies: Vec<ProxyDto>,
    pub server_info: Option<Value>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FrpSettingsInput {
    pub public_host: Option<String>,
    pub patch: Patch,
    /// Generate `auth.token` (32 random bytes, hex) and set `auth.method = "token"`.
    pub generate_token: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsSaved {
    pub changed: Vec<&'static str>,
    /// Masked line diff (`-`/`+`).
    pub diff: Vec<String>,
    pub pending_restart: bool,
    pub token_generated: bool,
}

#[derive(Clone)]
pub struct FrpService {
    ctx: Ctx,
    ports: IngressPorts,
    settings: IngressSettings,
    inspector: DaemonInspector,
    listeners: Arc<dyn HostProbePort>,
}

impl std::fmt::Debug for FrpService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrpService").finish_non_exhaustive()
    }
}

fn u64_of(v: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_u64))
        .unwrap_or(0)
}

fn str_of(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Lenient parse of `GET /api/proxy/{type}` (`{"proxies": [...]}`); shapes are unverified.
pub fn parse_proxies(ty: &str, body: &str) -> Vec<ProxyDto> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let list = v
        .get("proxies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    list.into_iter()
        .map(|p| {
            let conf = p.get("conf").filter(|c| c.is_object());
            let remote_port = conf.and_then(|c| {
                ["remotePort", "remote_port"]
                    .iter()
                    .find_map(|k| c.get(*k).and_then(Value::as_u64))
            });
            ProxyDto {
                name: str_of(&p, &["name"]).unwrap_or_default(),
                proxy_type: ty.to_owned(),
                status: str_of(&p, &["status"]).unwrap_or_else(|| "unknown".into()),
                remote_port,
                traffic_in: u64_of(&p, &["todayTrafficIn", "today_traffic_in"]),
                traffic_out: u64_of(&p, &["todayTrafficOut", "today_traffic_out"]),
                connections: u64_of(&p, &["curConns", "cur_conns"]),
                last_start: str_of(&p, &["lastStartTime", "last_start_time"]),
                last_close: str_of(&p, &["lastCloseTime", "last_close_time"]),
                client: str_of(&p, &["clientID", "client_id", "user"]),
                raw: p,
            }
        })
        .collect()
}

impl FrpService {
    pub(super) fn new(
        ctx: Ctx,
        ports: IngressPorts,
        settings: IngressSettings,
        inspector: DaemonInspector,
        listeners: Arc<dyn HostProbePort>,
    ) -> Self {
        Self {
            ctx,
            ports,
            settings,
            inspector,
            listeners,
        }
    }

    /// Config path: an explicit `config.toml` value wins; else `-c` from `ExecStart`; else the default.
    fn config_path(&self, daemon: &DaemonDto) -> PathBuf {
        let configured = &self.settings.frp_config_path;
        if configured.as_os_str() != frps::DEFAULT_CONFIG_PATH {
            return configured.clone();
        }
        daemon
            .config_path
            .as_ref()
            .map_or_else(|| configured.clone(), PathBuf::from)
    }

    async fn pending(&self) -> Result<bool, CoreError> {
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        Ok(settings::get(&mut conn, FRPS_PENDING_KEY).await?.is_some())
    }

    fn view(
        path: &std::path::Path,
        doc: &toml_edit::DocumentMut,
        mode: u32,
        o: &Owned,
    ) -> FrpConfigView {
        FrpConfigView {
            path: path.display().to_string(),
            bind_addr: o.bind_addr.clone(),
            bind_port: o.bind_port,
            auth_method: o.auth_method.clone(),
            auth_token_set: o.auth_token.is_some(),
            web_addr: o.web_addr.clone(),
            web_port: o.web_port,
            web_user: o.web_user.clone(),
            web_password_set: o.web_password.is_some(),
            allow_ports: o.allow_ports.clone(),
            dashboard_exposed: o.dashboard_exposed(),
            world_readable: frps::world_readable(mode),
            unowned_raw: frps::unowned_view(doc),
        }
    }

    /// `frps_overview`: daemon state, config, and (when the dashboard answers) live proxies.
    pub async fn overview(&self, actor: &Actor) -> Result<FrpOverview, CoreError> {
        actor.authorize(&OP_FRPS_OVERVIEW, None)?;
        let daemon = self.inspector.inspect(Daemon::Frps).await?;
        let path = self.config_path(&daemon);
        let mut warnings = Vec::new();
        let (mut config, mut config_error, mut owned) = (None, None, None);
        let mut backup_available = false;
        if daemon.installed {
            match self.ports.frp_files.read(&path)? {
                None => config_error = Some(format!("config file {} not found", path.display())),
                Some(f) => match frps::parse(&f.content) {
                    Err(e) => config_error = Some(e.message),
                    Ok(doc) => {
                        let o = frps::read_owned(&doc);
                        config = Some(Self::view(&path, &doc, f.mode, &o));
                        owned = Some(o);
                    }
                },
            }
            backup_available = self.ports.frp_files.read_backup(&path)?.is_some();
        }
        if let Some(c) = &config {
            if c.world_readable {
                warnings.push("frps.toml is world-readable and holds secrets; chmod 600 it".into());
            }
            if c.dashboard_exposed {
                warnings
                    .push("the frps dashboard listens on a non-loopback address (exposed)".into());
            }
            if c.allow_ports.is_empty() {
                warnings.push(
                    "allowPorts is empty: clients may open any remote port (unverified semantics)"
                        .into(),
                );
            }
        }
        let mut status = match daemon.state {
            DaemonState::NotInstalled => FrpStatus::NotInstalled,
            DaemonState::Running => FrpStatus::Running,
            _ => FrpStatus::InstalledInactive,
        };
        let (mut proxies, mut server_info, mut version) =
            (Vec::new(), None, daemon.version.clone());
        if status == FrpStatus::Running {
            match owned.as_ref().filter(|o| o.web_port.is_some()) {
                None => status = FrpStatus::RunningNoDashboard,
                Some(o) => {
                    let addr = match o.dashboard_addr().as_str() {
                        "0.0.0.0" | "::" => "127.0.0.1".to_owned(),
                        a => a.to_owned(),
                    };
                    let auth = match (&o.web_user, &o.web_password) {
                        (Some(u), Some(p)) if !u.is_empty() => {
                            Some((u.clone(), Secret::new(p.expose())))
                        }
                        _ => None,
                    };
                    let target = DashboardTarget {
                        addr,
                        port: o.web_port.unwrap_or_default(),
                        auth,
                    };
                    match self
                        .ports
                        .frp_dashboard
                        .get(&target, "/api/serverinfo")
                        .await
                    {
                        Err(_) => status = FrpStatus::DashboardUnreachable,
                        Ok(a) if a.status == 401 => status = FrpStatus::DashboardAuthFailed,
                        Ok(a) => {
                            if let Ok(v) = serde_json::from_str::<Value>(&a.body) {
                                if version.is_none() {
                                    version = str_of(&v, &["version"]);
                                }
                                server_info = Some(v);
                            }
                            for ty in PROXY_TYPES {
                                if let Ok(r) = self
                                    .ports
                                    .frp_dashboard
                                    .get(&target, &format!("/api/proxy/{ty}"))
                                    .await
                                    && r.status == 200
                                {
                                    proxies.extend(parse_proxies(ty, &r.body));
                                }
                            }
                        }
                    }
                }
            }
        }
        let public_host = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            repo::frps_public_host(&mut conn).await?
        };
        Ok(FrpOverview {
            status,
            status_label: status.label(),
            daemon,
            version,
            config,
            config_error,
            pending_restart: self.pending().await?,
            backup_available,
            public_host,
            proxies,
            server_info,
            warnings,
        })
    }

    /// Ports that cannot be used: a Host listener other than frps itself, or a declared Project port.
    async fn port_conflicts(
        &self,
        field: &str,
        port: u16,
        frps_pid_ports: &[u16],
        errs: &mut Vec<FieldError>,
    ) -> Result<(), CoreError> {
        if frps_pid_ports.contains(&port) {
            return Ok(());
        }
        let hit = self
            .listeners
            .listeners()
            .await?
            .into_iter()
            .find(|l| l.port == port && l.protocol == "tcp");
        if let Some(l) = hit {
            errs.push(FieldError {
                field: field.to_owned(),
                message: format!(
                    "port {port} is already in use by {}",
                    l.process.as_deref().unwrap_or("another process")
                ),
            });
        }
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let declared: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM project_ports WHERE host_port = ?")
                .bind(i64::from(port))
                .fetch_one(&mut *conn)
                .await
                .map_err(db_err)?;
        if declared > 0 {
            errs.push(FieldError {
                field: field.to_owned(),
                message: format!("port {port} is declared by a Project"),
            });
        }
        Ok(())
    }

    /// `frps_settings_update`: change owned fields, preserve the rest, write atomically with a
    /// `.bak`. Does not restart; the UI/REST then offers `daemon_control restart` (D28).
    pub async fn settings_update(
        &self,
        actor: &Actor,
        input: &FrpSettingsInput,
    ) -> Result<SettingsSaved, CoreError> {
        actor.authorize(&OP_FRPS_SETTINGS_UPDATE, None)?;
        let daemon = self.inspector.inspect(Daemon::Frps).await?;
        let path = self.config_path(&daemon);
        let file = self.ports.frp_files.read(&path)?.ok_or_else(|| {
            CoreError::conflict(format!(
                "{} does not exist; create it with the frps installation first",
                path.display()
            ))
        })?;
        let mut doc = frps::parse(&file.content)?;
        let current = frps::read_owned(&doc);

        let mut patch = input.patch.clone();
        let token_generated = input.generate_token;
        if token_generated {
            patch.auth_token = Change::Set(Secret::new(crate::crypto::random_hex::<32>()));
            patch.auth_method = Change::Set("token".into());
        }
        let next = frps::resolve(&current, &patch);
        next.validate()?;

        // Port collisions for ports that changed (frps's own current ports are not collisions).
        let own: Vec<u16> = [current.bind_port, current.web_port]
            .into_iter()
            .flatten()
            .collect();
        let mut errs = Vec::new();
        if next.bind_port != current.bind_port
            && let Some(p) = next.bind_port
        {
            self.port_conflicts("bind_port", p, &own, &mut errs).await?;
        }
        if next.web_port != current.web_port
            && let Some(p) = next.web_port
        {
            self.port_conflicts("web_port", p, &own, &mut errs).await?;
        }
        if !errs.is_empty() {
            let msg = errs
                .iter()
                .map(|e| e.message.clone())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(CoreError::new(crate::ErrorKind::Validation(errs), msg));
        }

        let mut changed: Vec<&'static str> = Vec::new();
        macro_rules! diff {
            ($name:literal, $a:expr, $b:expr) => {
                if $a != $b {
                    changed.push($name);
                }
            };
        }
        diff!("bindAddr", current.bind_addr, next.bind_addr);
        diff!("bindPort", current.bind_port, next.bind_port);
        diff!("auth.method", current.auth_method, next.auth_method);
        diff!(
            "auth.token",
            current.auth_token.as_ref().map(Secret::expose),
            next.auth_token.as_ref().map(Secret::expose)
        );
        diff!("webServer.addr", current.web_addr, next.web_addr);
        diff!("webServer.port", current.web_port, next.web_port);
        diff!("webServer.user", current.web_user, next.web_user);
        diff!(
            "webServer.password",
            current.web_password.as_ref().map(Secret::expose),
            next.web_password.as_ref().map(Secret::expose)
        );
        diff!("allowPorts", current.allow_ports, next.allow_ports);

        let mut public_host_changed = false;
        let new_host = input.public_host.as_deref().map(str::trim);
        if let Some(h) = new_host {
            if h.len() > 253 || h.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(CoreError::validation("public_host", "invalid host"));
            }
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            public_host_changed = repo::frps_public_host(&mut conn).await? != h;
        }

        let mut diff_lines = Vec::new();
        if !changed.is_empty() {
            frps::apply(&mut doc, &patch);
            let text = doc.to_string();
            // The result must still parse and still read back as what was validated.
            let back = frps::read_owned(&frps::parse(&text)?);
            back.validate()?;
            diff_lines = frps::masked_diff(&file.content, &text);
            self.ports.frp_files.write(&path, &text)?;
        }
        if changed.is_empty() && !public_host_changed {
            return Ok(SettingsSaved {
                changed,
                diff: diff_lines,
                pending_restart: self.pending().await?,
                token_generated: false,
            });
        }
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        if public_host_changed {
            repo::frps_set_public_host(&mut tx, new_host.unwrap_or_default(), now).await?;
        }
        if !changed.is_empty() {
            settings::set(&mut tx, FRPS_PENDING_KEY, "true", now).await?;
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            // Field names only: no values (secrets live in frps.toml alone).
            &AuditEvent::new("frps.settings_update", "frps")
                .meta(json!({ "fields": changed, "public_host_changed": public_host_changed, "token_generated": token_generated })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(SettingsSaved {
            pending_restart: self.pending().await?,
            changed,
            diff: diff_lines,
            token_generated,
        })
    }

    /// Put the `.bak` of the previous config back (the Owner then restarts frps again).
    pub async fn config_restore(&self, actor: &Actor) -> Result<(), CoreError> {
        actor.authorize(&OP_FRPS_CONFIG_RESTORE, None)?;
        let daemon = self.inspector.inspect(Daemon::Frps).await?;
        let path = self.config_path(&daemon);
        let backup = self
            .ports
            .frp_files
            .read_backup(&path)?
            .ok_or_else(|| CoreError::conflict("there is no previous config to restore"))?;
        frps::parse(&backup)?;
        self.ports.frp_files.write(&path, &backup)?;
        let mut tx = self.ctx.db.begin_write().await?;
        settings::set(&mut tx, FRPS_PENDING_KEY, "true", self.ctx.now()).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("frps.config_restore", "frps").meta(json!({})),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    /// Show `auth.token` once (dashboard only). Audited; never part of the audit row.
    pub async fn token_reveal(&self, actor: &Actor) -> Result<Secret, CoreError> {
        actor.authorize(&OP_FRPS_TOKEN_REVEAL, None)?;
        let daemon = self.inspector.inspect(Daemon::Frps).await?;
        let path = self.config_path(&daemon);
        let file = self
            .ports
            .frp_files
            .read(&path)?
            .ok_or_else(|| CoreError::not_found("frps.toml not found"))?;
        let token = frps::read_owned(&frps::parse(&file.content)?)
            .auth_token
            .ok_or_else(|| CoreError::not_found("frps.toml has no auth.token"))?;
        audit::write_standalone(
            &self.ctx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("frps.token_reveal", "frps").meta(json!({})),
        )
        .await?;
        Ok(token)
    }
}
