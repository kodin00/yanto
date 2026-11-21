//! Host Daemons (docs/rewrite/04 8.3, 9.2, 9.3; ADR 0006): status of `cloudflared`, `traefik` and
//! `frps` through systemd, install instructions when missing, and `daemon_control`
//! (start/stop/restart behind an explicit confirmation). Yanto never installs a daemon.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::json;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use crate::actor::{Access, Actor, OpSpec, Scope};
use crate::config::Config;
use crate::db::repo::settings;
use crate::error::CoreError;
use crate::ingress::daemon::{
    Daemon, DaemonState, FRPS_SAMPLE_UNIT, config_path_from_args, install_instructions,
    parse_exec_start, valid_unit_name,
};
use crate::ports::{IngressPorts, UnitAction, UnitProps};

pub const OP_DAEMONS_STATUS: OpSpec = OpSpec::new("daemons_status", Access::Owner, Scope::Read);
pub const OP_DAEMON_CONTROL: OpSpec = OpSpec::new("daemon_control", Access::Owner, Scope::Admin);

pub const OPERATIONS: &[&OpSpec] = &[&OP_DAEMONS_STATUS, &OP_DAEMON_CONTROL];

/// `app_settings` key: the `frps` config was saved and not yet applied by a restart (04 8.4).
pub const FRPS_PENDING_KEY: &str = "frps.pending_restart";

/// The `config.toml` values M7 needs (host paths and unit names), copied once at startup.
#[derive(Debug, Clone)]
pub struct IngressSettings {
    pub cloudflared_unit: String,
    pub traefik_unit: String,
    pub traefik_dynamic_dir: PathBuf,
    pub traefik_static_config: PathBuf,
    pub cert_resolver: String,
    pub entrypoint: String,
    pub frp_unit: String,
    pub frp_config_path: PathBuf,
}

impl IngressSettings {
    pub fn from_config(c: &Config) -> Self {
        Self {
            cloudflared_unit: c.ingress.cloudflared.unit.clone(),
            traefik_unit: c.ingress.traefik.unit.clone(),
            traefik_dynamic_dir: c.ingress.traefik.dynamic_dir.clone(),
            traefik_static_config: c.ingress.traefik.static_config.clone(),
            cert_resolver: c.ingress.traefik.cert_resolver.clone(),
            entrypoint: c.ingress.traefik.entrypoint.clone(),
            frp_unit: c.frp.unit.clone(),
            frp_config_path: c.frp.config_path.clone(),
        }
    }

    pub fn unit(&self, d: Daemon) -> &str {
        match d {
            Daemon::Cloudflared => &self.cloudflared_unit,
            Daemon::Traefik => &self.traefik_unit,
            Daemon::Frps => &self.frp_unit,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DaemonDto {
    pub name: &'static str,
    pub unit: String,
    pub state: DaemonState,
    pub state_label: &'static str,
    pub installed: bool,
    pub active: bool,
    pub version: Option<String>,
    pub unit_file: Option<String>,
    pub binary: Option<String>,
    /// From `ExecStart` (`-c`/`--config`), when present.
    pub config_path: Option<String>,
    /// Shown only when the daemon is not installed.
    pub instructions: Vec<String>,
    pub sample_unit: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ControlOutcome {
    pub name: &'static str,
    pub action: &'static str,
    pub state: DaemonState,
}

/// Shared by the daemon, FRP and Hostname services.
#[derive(Clone)]
pub struct DaemonInspector {
    pub ports: IngressPorts,
    pub settings: IngressSettings,
}

impl DaemonInspector {
    pub fn unit_of(&self, d: Daemon) -> Result<String, CoreError> {
        let unit = self.settings.unit(d);
        if valid_unit_name(unit) {
            Ok(unit.to_owned())
        } else {
            Err(CoreError::validation(
                "unit",
                format!(
                    "{} unit name {unit:?} in config.toml is invalid",
                    d.as_str()
                ),
            ))
        }
    }

    pub async fn props(&self, d: Daemon) -> Result<(String, UnitProps), CoreError> {
        let unit = self.unit_of(d)?;
        let props = self.ports.systemd.show(&unit).await?;
        Ok((unit, props))
    }

    pub async fn inspect(&self, d: Daemon) -> Result<DaemonDto, CoreError> {
        let (unit, props) = self.props(d).await?;
        let state = DaemonState::from_props(&props);
        let exec = parse_exec_start(&props.exec_start);
        let (default_bin, args) = d.version_args();
        let bin = exec
            .as_ref()
            .map(|(b, _)| b.clone())
            .unwrap_or_else(|| default_bin.to_owned());
        let version = self.ports.systemd.binary_version(&bin, args).await;
        let installed = state != DaemonState::NotInstalled;
        Ok(DaemonDto {
            name: d.as_str(),
            state,
            state_label: state.label(),
            installed,
            active: state == DaemonState::Running,
            version,
            unit_file: props.fragment_path.clone(),
            binary: exec.as_ref().map(|(b, _)| b.clone()),
            config_path: exec.as_ref().and_then(|(_, a)| config_path_from_args(a)),
            instructions: if installed {
                Vec::new()
            } else {
                install_instructions(d, &unit)
            },
            sample_unit: (!installed && d == Daemon::Frps).then_some(FRPS_SAMPLE_UNIT),
            unit,
        })
    }
}

#[derive(Clone)]
pub struct DaemonService {
    ctx: Ctx,
    pub(super) inspector: DaemonInspector,
}

impl std::fmt::Debug for DaemonService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonService").finish_non_exhaustive()
    }
}

impl DaemonService {
    pub(super) fn new(ctx: Ctx, inspector: DaemonInspector) -> Self {
        Self { ctx, inspector }
    }

    pub fn inspector(&self) -> &DaemonInspector {
        &self.inspector
    }

    /// `daemons_status`: installed/running/version of all three daemons plus install hints.
    pub async fn status(&self, actor: &Actor) -> Result<Vec<DaemonDto>, CoreError> {
        actor.authorize(&OP_DAEMONS_STATUS, None)?;
        let mut out = Vec::with_capacity(3);
        for d in Daemon::ALL {
            out.push(self.inspector.inspect(d).await?);
        }
        Ok(out)
    }

    /// `daemon_control`: `systemctl start|stop|restart <unit>` behind `confirm`. Restarting
    /// `frps` drops every live tunnel (D28); the caller must have shown that warning.
    pub async fn control(
        &self,
        actor: &Actor,
        name: &str,
        action: &str,
        confirm: bool,
    ) -> Result<ControlOutcome, CoreError> {
        actor.authorize(&OP_DAEMON_CONTROL, None)?;
        let d = Daemon::parse(name)?;
        let act = match action {
            "start" => UnitAction::Start,
            "stop" => UnitAction::Stop,
            "restart" => UnitAction::Restart,
            _ => {
                return Err(CoreError::validation(
                    "action",
                    "action must be start, stop or restart",
                ));
            }
        };
        if !confirm {
            let extra = if d == Daemon::Frps && act != UnitAction::Start {
                " This drops every live frp tunnel."
            } else {
                ""
            };
            return Err(CoreError::validation(
                "confirm",
                format!("confirmation required to {action} {name}.{extra}"),
            ));
        }
        let (unit, props) = self.inspector.props(d).await?;
        if DaemonState::from_props(&props) == DaemonState::NotInstalled {
            return Err(CoreError::conflict(format!(
                "{name} is not installed (unit {unit} not found); install it first"
            )));
        }
        let res = self.inspector.ports.systemd.control(&unit, act).await?;
        let audit_actor = AuditActor::from_actor(actor);
        if !res.ok {
            let status = self
                .inspector
                .ports
                .systemd
                .status_text(&unit)
                .await
                .unwrap_or_default();
            let journal = self
                .inspector
                .ports
                .systemd
                .journal_tail(&unit, 50)
                .await
                .unwrap_or_default();
            audit::write_standalone(
                &self.ctx,
                &audit_actor,
                &AuditEvent::new("daemon.control", "daemon")
                    .entity(name)
                    .meta(json!({ "action": action, "unit": unit, "ok": false })),
            )
            .await?;
            return Err(CoreError::conflict(format!(
                "systemctl {action} {unit} failed: {}\n--- status ---\n{status}\n--- journal ---\n{journal}",
                res.output.trim()
            )));
        }
        let state = DaemonState::from_props(&self.inspector.ports.systemd.show(&unit).await?);
        let mut tx = self.ctx.db.begin_write().await?;
        if d == Daemon::Frps && act != UnitAction::Stop && state == DaemonState::Running {
            settings::delete(&mut tx, FRPS_PENDING_KEY).await?;
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &audit_actor,
            &AuditEvent::new("daemon.control", "daemon")
                .entity(name)
                .meta(json!({ "action": action, "unit": unit, "ok": true })),
        )
        .await?;
        tx.commit().await.map_err(crate::db::repo::db_err)?;
        Ok(ControlOutcome {
            name: d.as_str(),
            action: act.as_str(),
            state,
        })
    }
}
