//! Hostnames and Ingress Providers (docs/rewrite/04 9.1-9.5, ADR 0008, D16).
//!
//! SQLite is the desired state. Applying a provider regenerates its entire config from the DB and
//! pushes it idempotently: a full `PUT` of the cloudflared ingress list, or a full rewrite of the
//! Traefik `yanto.yml`. A manual "Re-sync" does the same for everything.

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use super::cloudflare::{CloudflareService, cf_err};
use super::daemons::{DaemonDto, DaemonInspector, IngressSettings};
use crate::actor::{Access, Actor, Need, OpSpec, Permission, Scope};
use crate::crypto::{new_id, sha256_hex};
use crate::db::repo::ingress as repo;
use crate::db::repo::{db_err, settings};
use crate::error::CoreError;
use crate::ingress::cloudflare::{MANAGED_COMMENT, build_ingress, cname_target, drifted};
use crate::ingress::daemon::Daemon;
use crate::ingress::traefik::{self, CheckItem, CheckStatus, Route};
use crate::ingress::{Provider, in_zone, normalize_hostname};
use crate::ports::{CfAuth, CfDnsRecord, DnsRecordInput, IngressPorts};
use crate::secrets::Secret;

pub const OP_HOSTNAMES_LIST: OpSpec =
    OpSpec::new("hostnames_list", Access::Authenticated, Scope::Read);
pub const OP_HOSTNAME_CREATE: OpSpec = OpSpec::new(
    "hostname_create",
    Access::Project(Need::Permission(Permission::Hostnames)),
    Scope::Write,
);
pub const OP_HOSTNAME_DELETE: OpSpec = OpSpec::new(
    "hostname_delete",
    Access::Project(Need::Permission(Permission::Hostnames)),
    Scope::Admin,
);
pub const OP_HOSTNAME_RETRY: OpSpec = OpSpec::new(
    "hostname_retry",
    Access::Project(Need::Permission(Permission::Hostnames)),
    Scope::Write,
);
pub const OP_HOSTNAME_ENABLE: OpSpec = OpSpec::new(
    "hostname_enable",
    Access::Project(Need::Permission(Permission::Hostnames)),
    Scope::Write,
);
pub const OP_HOSTNAME_DISABLE: OpSpec = OpSpec::new(
    "hostname_disable",
    Access::Project(Need::Permission(Permission::Hostnames)),
    Scope::Write,
);
/// Create/delete/retry/enable/disable of a Hostname that targets the Hosted Registry: Owner only.
pub const OP_HOSTNAME_REGISTRY_MANAGE: OpSpec =
    OpSpec::new("hostname_registry_manage", Access::Owner, Scope::Admin);
pub const OP_HOSTNAMES_DIAGNOSTICS: OpSpec =
    OpSpec::new("hostnames_diagnostics", Access::Authenticated, Scope::Read);
pub const OP_HOSTNAMES_RESYNC: OpSpec =
    OpSpec::new("hostnames_resync", Access::Owner, Scope::Write);
pub const OP_TRAEFIK_STATUS: OpSpec = OpSpec::new("traefik_status", Access::Owner, Scope::Read);
/// What a Member may choose when creating a Hostname (the Owner configured it): readiness of each
/// provider, the active tunnel, and the zones of its client.
pub const OP_HOSTNAMES_OPTIONS: OpSpec =
    OpSpec::new("hostnames_options", Access::Authenticated, Scope::Read);
pub const OP_HOSTNAMES_PUBLIC_IP_SET: OpSpec =
    OpSpec::new("hostnames_public_ip_set", Access::Owner, Scope::Admin);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_HOSTNAMES_LIST,
    &OP_HOSTNAME_CREATE,
    &OP_HOSTNAME_DELETE,
    &OP_HOSTNAME_RETRY,
    &OP_HOSTNAME_ENABLE,
    &OP_HOSTNAME_DISABLE,
    &OP_HOSTNAME_REGISTRY_MANAGE,
    &OP_HOSTNAMES_DIAGNOSTICS,
    &OP_HOSTNAMES_RESYNC,
    &OP_TRAEFIK_STATUS,
    &OP_HOSTNAMES_PUBLIC_IP_SET,
    &OP_HOSTNAMES_OPTIONS,
];

const TRAEFIK_HASH_KEY: &str = "traefik.file_sha256";
const VERIFY_WAIT_SECS: u64 = 10;

#[derive(Debug, Clone)]
pub enum TargetInput {
    Project {
        project_id: String,
        project_port_id: String,
    },
    /// The Hosted Registry (Owner only).
    Registry,
}

#[derive(Debug, Clone)]
pub struct HostnameInput {
    pub target: TargetInput,
    pub hostname: String,
    /// `cloudflare_tunnel` or `traefik`.
    pub ingress_provider: String,
    /// Required for `cloudflare_tunnel` (the `cft_` id).
    pub tunnel_id: Option<String>,
    pub zone_id: Option<String>,
    pub manage_dns: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostnameDto {
    pub id: String,
    pub target_kind: String,
    pub project_id: Option<String>,
    pub project_slug: Option<String>,
    pub project_port_id: Option<String>,
    pub hostname: String,
    pub ingress_provider: String,
    pub tunnel_id: Option<String>,
    pub zone_id: Option<String>,
    pub manage_dns: bool,
    pub dns_record_id: Option<String>,
    pub enabled: bool,
    pub sync_status: String,
    pub last_error: Option<String>,
    pub last_published_at: Option<i64>,
    pub host_port: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<repo::HostnameRow> for HostnameDto {
    fn from(r: repo::HostnameRow) -> Self {
        Self {
            id: r.id,
            target_kind: r.target_kind,
            project_id: r.project_id,
            project_slug: r.project_slug,
            project_port_id: r.project_port_id,
            hostname: r.hostname,
            ingress_provider: r.ingress_provider,
            tunnel_id: r.tunnel_id,
            zone_id: r.zone_id,
            manage_dns: r.manage_dns,
            dns_record_id: r.dns_record_id,
            enabled: r.enabled,
            sync_status: r.sync_status,
            last_error: r.last_error,
            last_published_at: r.last_published_at,
            host_port: r.host_port,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PublishOutcome {
    pub hostname: HostnameDto,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteOutcome {
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResyncReport {
    pub hostnames: usize,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagCheck {
    pub name: &'static str,
    pub status: DiagStatus,
    pub detail: String,
    pub hint: Option<String>,
}

impl DiagCheck {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: DiagStatus::Pass,
            detail: detail.into(),
            hint: None,
        }
    }
    fn warn(name: &'static str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            name,
            status: DiagStatus::Warn,
            detail: detail.into(),
            hint: Some(hint.into()),
        }
    }
    fn fail(name: &'static str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            name,
            status: DiagStatus::Fail,
            detail: detail.into(),
            hint: Some(hint.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HostnameDiagnostics {
    pub hostname: HostnameDto,
    pub checks: Vec<DiagCheck>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    InSync,
    /// Edited outside Yanto, or the DB changed and the file is stale.
    Drift,
    Missing,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraefikStatus {
    pub daemon: DaemonDto,
    pub dynamic_dir: String,
    pub dir_exists: bool,
    pub dir_writable: bool,
    pub checklist: Vec<CheckItem>,
    pub ready: bool,
    pub not_ready_reason: Option<String>,
    pub file_state: FileState,
    /// `Some(false)`: `watch` is off, so a change needs a Traefik restart.
    pub watch: Option<bool>,
    pub hostnames: usize,
    pub entrypoint: String,
    pub cert_resolver: String,
    /// Static configuration the operator must provide (shown, never written).
    pub prerequisites: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostnameOptions {
    pub traefik_ready: bool,
    pub traefik_reason: Option<String>,
    /// The tunnel bound to the Host `cloudflared`; used when a Hostname names no tunnel.
    pub tunnel: Option<ActiveTunnel>,
    pub zones: Vec<super::cloudflare::ZoneDto>,
    pub public_ip_set: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveTunnel {
    pub id: String,
    pub name: String,
}

#[derive(Clone)]
pub struct HostnameService {
    ctx: Ctx,
    ports: IngressPorts,
    settings: IngressSettings,
    inspector: DaemonInspector,
    cloudflare: CloudflareService,
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for HostnameService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostnameService").finish_non_exhaustive()
    }
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(600).collect();
    t.trim().to_owned()
}

impl HostnameService {
    pub(super) fn new(
        ctx: Ctx,
        ports: IngressPorts,
        settings: IngressSettings,
        inspector: DaemonInspector,
        cloudflare: CloudflareService,
    ) -> Self {
        Self {
            ctx,
            ports,
            settings,
            inspector,
            cloudflare,
            lock: Arc::default(),
        }
    }

    async fn reader(&self) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, CoreError> {
        self.ctx.db.reader().acquire().await.map_err(db_err)
    }

    /// Token scope is checked before the row is loaded, so a too-weak token learns nothing
    /// about which ids exist (403, not 404).
    fn scope_guard(actor: &Actor, op: &OpSpec) -> Result<(), CoreError> {
        match actor.token_scope() {
            Some(sc) if !op.token_allowed || sc < op.min_scope => {
                Err(CoreError::forbidden(format!(
                    "{} requires an API Token with {} scope",
                    op.name, op.min_scope
                )))
            }
            _ => Ok(()),
        }
    }

    /// Authorization for an existing row: the Project permission, or Owner/admin for the registry.
    fn authorize_row(
        &self,
        actor: &Actor,
        op: &OpSpec,
        row: &repo::HostnameRow,
    ) -> Result<(), CoreError> {
        if row.target_kind == "registry" {
            actor.authorize(&OP_HOSTNAME_REGISTRY_MANAGE, None)
        } else {
            actor.authorize(op, row.project_id.as_deref())
        }
    }

    /// Rows `actor` may see: all for the Owner, else Projects where it holds `hostnames`.
    fn visible(&self, actor: &Actor, row: &repo::HostnameRow) -> bool {
        if actor.is_owner() {
            return true;
        }
        row.project_id
            .as_ref()
            .and_then(|p| actor.grants.get(p))
            .is_some_and(|perms| perms.contains(&Permission::Hostnames))
    }

    async fn load(&self, id: &str) -> Result<repo::HostnameRow, CoreError> {
        let mut conn = self.reader().await?;
        repo::hostname_get(&mut conn, id)
            .await?
            .ok_or_else(|| CoreError::not_found("hostname not found"))
    }

    // ---- reads ---------------------------------------------------------------------------

    pub async fn list(
        &self,
        actor: &Actor,
        project: Option<&str>,
    ) -> Result<Vec<HostnameDto>, CoreError> {
        actor.authorize(&OP_HOSTNAMES_LIST, None)?;
        if let Some(p) = project
            && !actor.is_owner()
        {
            match actor.grants.get(p) {
                None => return Err(CoreError::not_found("project not found")),
                Some(perms) if !perms.contains(&Permission::Hostnames) => {
                    return Err(CoreError::forbidden(
                        "requires the hostnames permission on this project",
                    ));
                }
                Some(_) => {}
            }
        }
        let mut conn = self.reader().await?;
        let rows = match project {
            Some(p) => repo::hostname_list_for_project(&mut conn, p).await?,
            None => repo::hostname_list(&mut conn).await?,
        };
        Ok(rows
            .into_iter()
            .filter(|r| self.visible(actor, r))
            .map(Into::into)
            .collect())
    }

    /// Provider readiness, the active tunnel and its zones (from the cache; no API calls).
    pub async fn options(&self, actor: &Actor) -> Result<HostnameOptions, CoreError> {
        actor.authorize(&OP_HOSTNAMES_OPTIONS, None)?;
        let t = self.traefik_status_inner().await?;
        let mut conn = self.reader().await?;
        let tunnel = repo::tunnel_active(&mut conn).await?;
        let zones = match &tunnel {
            Some(t) => repo::zones_for_client(&mut conn, &t.client_id)
                .await?
                .into_iter()
                .map(Into::into)
                .collect(),
            None => Vec::new(),
        };
        let (v4, v6) = CloudflareService::public_ips(&mut conn).await?;
        Ok(HostnameOptions {
            traefik_ready: t.ready,
            traefik_reason: t.not_ready_reason,
            tunnel: tunnel.map(|t| ActiveTunnel {
                id: t.id,
                name: t.name,
            }),
            zones,
            public_ip_set: v4.is_some() || v6.is_some(),
        })
    }

    // ---- create --------------------------------------------------------------------------

    pub async fn create(
        &self,
        actor: &Actor,
        input: &HostnameInput,
    ) -> Result<PublishOutcome, CoreError> {
        let (project, port_id) = match &input.target {
            TargetInput::Project {
                project_id,
                project_port_id,
            } => {
                actor.authorize(&OP_HOSTNAME_CREATE, Some(project_id))?;
                (Some(project_id.as_str()), Some(project_port_id.as_str()))
            }
            TargetInput::Registry => {
                actor.authorize(&OP_HOSTNAME_REGISTRY_MANAGE, None)?;
                (None, None)
            }
        };
        let hostname = normalize_hostname(&input.hostname)?;
        let provider = Provider::parse(&input.ingress_provider)?;

        // Target validation.
        {
            let mut conn = self.reader().await?;
            match (project, port_id) {
                (Some(p), Some(pp)) => {
                    let exists: i64 =
                        sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE id = ?")
                            .bind(p)
                            .fetch_one(&mut *conn)
                            .await
                            .map_err(db_err)?;
                    if exists == 0 {
                        return Err(CoreError::not_found("project not found"));
                    }
                    let port = repo::port_of_project(&mut conn, p, pp)
                        .await?
                        .ok_or_else(|| {
                            CoreError::validation(
                                "project_port_id",
                                "the port is not declared by this project",
                            )
                        })?;
                    if port.bind_address != "127.0.0.1" || port.protocol != "tcp" {
                        return Err(CoreError::validation(
                            "project_port_id",
                            "only TCP ports published on 127.0.0.1 can be exposed",
                        ));
                    }
                }
                _ => {
                    if self.ports.registry.listen_port().await?.is_none() {
                        return Err(CoreError::validation(
                            "target",
                            "the Hosted Registry is not enabled",
                        ));
                    }
                }
            }
        }

        // Provider validation.
        let mut tunnel_id = None;
        let mut zone_id = input.zone_id.clone().filter(|z| !z.is_empty());
        match provider {
            Provider::CloudflareTunnel => {
                let mut conn = self.reader().await?;
                let t = match input.tunnel_id.clone().filter(|t| !t.is_empty()) {
                    Some(t) => t,
                    // No tunnel named: use the one bound to the Host cloudflared.
                    None => repo::tunnel_active(&mut conn)
                        .await?
                        .map(|t| t.id)
                        .ok_or_else(|| {
                            CoreError::validation(
                                "tunnel_id",
                                "no active Cloudflare tunnel; the Owner must create one first",
                            )
                        })?,
                };
                let tunnel = repo::tunnel_get(&mut conn, &t)
                    .await?
                    .ok_or_else(|| CoreError::validation("tunnel_id", "tunnel not found"))?;
                if let Some(z) = &zone_id {
                    let zone = repo::zone_get(&mut conn, z)
                        .await?
                        .filter(|z| z.client_id == tunnel.client_id)
                        .ok_or_else(|| {
                            CoreError::validation(
                                "zone_id",
                                "zone not found for the tunnel's client",
                            )
                        })?;
                    if !in_zone(&hostname, &zone.name) {
                        return Err(CoreError::validation(
                            "hostname",
                            format!("hostname is not inside zone {}", zone.name),
                        ));
                    }
                }
                tunnel_id = Some(t);
            }
            Provider::Traefik => {
                let st = self.traefik_status_inner().await?;
                if !st.ready {
                    return Err(CoreError::validation(
                        "ingress_provider",
                        format!(
                            "Traefik is not ready: {}",
                            st.not_ready_reason.unwrap_or_default()
                        ),
                    ));
                }
                if let Some(z) = &zone_id {
                    let mut conn = self.reader().await?;
                    let zone = repo::zone_get(&mut conn, z)
                        .await?
                        .ok_or_else(|| CoreError::validation("zone_id", "zone not found"))?;
                    if !in_zone(&hostname, &zone.name) {
                        return Err(CoreError::validation(
                            "hostname",
                            format!("hostname is not inside zone {}", zone.name),
                        ));
                    }
                }
            }
        }
        if input.manage_dns {
            if zone_id.is_none() {
                return Err(CoreError::validation(
                    "zone_id",
                    "choose a zone to manage DNS",
                ));
            }
            if provider == Provider::Traefik {
                let mut conn = self.reader().await?;
                let (v4, v6) = CloudflareService::public_ips(&mut conn).await?;
                if v4.is_none() && v6.is_none() {
                    return Err(CoreError::validation(
                        "manage_dns",
                        "set the Host public IP before Yanto can create A/AAAA records",
                    ));
                }
            }
        } else if provider == Provider::Traefik {
            zone_id = zone_id.filter(|_| false);
        }

        let id = new_id("hst");
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        repo::hostname_insert(
            &mut tx,
            &repo::NewHostname {
                id: &id,
                target_kind: if project.is_some() {
                    "project"
                } else {
                    "registry"
                },
                project_id: project,
                project_port_id: port_id,
                hostname: &hostname,
                provider: provider.as_str(),
                tunnel_id: tunnel_id.as_deref(),
                zone_id: zone_id.as_deref(),
                manage_dns: input.manage_dns,
            },
            now,
        )
        .await?;
        let mut ev = AuditEvent::new("hostname.create", "hostname")
            .entity(&id)
            .meta(json!({
                "hostname": hostname, "provider": provider.as_str(), "manage_dns": input.manage_dns,
                "target": if project.is_some() { "project" } else { "registry" },
            }));
        if let Some(p) = project {
            ev = ev.project(p);
        }
        audit::write(&self.ctx, &mut tx, &AuditActor::from_actor(actor), &ev).await?;
        tx.commit().await.map_err(db_err)?;
        self.publish(&id).await
    }

    // ---- state changes -------------------------------------------------------------------

    pub async fn retry(&self, actor: &Actor, id: &str) -> Result<PublishOutcome, CoreError> {
        Self::scope_guard(actor, &OP_HOSTNAME_RETRY)?;
        let row = self.load(id).await?;
        self.authorize_row(actor, &OP_HOSTNAME_RETRY, &row)?;
        self.publish(id).await
    }

    pub async fn enable(&self, actor: &Actor, id: &str) -> Result<PublishOutcome, CoreError> {
        self.set_enabled(actor, id, true).await
    }

    pub async fn disable(&self, actor: &Actor, id: &str) -> Result<PublishOutcome, CoreError> {
        self.set_enabled(actor, id, false).await
    }

    async fn set_enabled(
        &self,
        actor: &Actor,
        id: &str,
        enabled: bool,
    ) -> Result<PublishOutcome, CoreError> {
        let op = if enabled {
            &OP_HOSTNAME_ENABLE
        } else {
            &OP_HOSTNAME_DISABLE
        };
        Self::scope_guard(actor, op)?;
        let row = self.load(id).await?;
        self.authorize_row(actor, op, &row)?;
        let mut tx = self.ctx.db.begin_write().await?;
        repo::hostname_set_enabled(&mut tx, id, enabled, self.ctx.now()).await?;
        let mut ev = AuditEvent::new(
            if enabled {
                "hostname.enable"
            } else {
                "hostname.disable"
            },
            "hostname",
        )
        .entity(id)
        .meta(json!({ "hostname": row.hostname }));
        if let Some(p) = &row.project_id {
            ev = ev.project(p);
        }
        audit::write(&self.ctx, &mut tx, &AuditActor::from_actor(actor), &ev).await?;
        tx.commit().await.map_err(db_err)?;
        self.publish(id).await
    }

    /// Remove the Hostname from its provider first (a failure keeps the row so it can be retried),
    /// then delete the row. DNS cleanup problems are reported as warnings.
    pub async fn delete(&self, actor: &Actor, id: &str) -> Result<DeleteOutcome, CoreError> {
        Self::scope_guard(actor, &OP_HOSTNAME_DELETE)?;
        let row = self.load(id).await?;
        self.authorize_row(actor, &OP_HOSTNAME_DELETE, &row)?;
        let _g = self.lock.lock().await;
        let mut warnings = Vec::new();
        let rows = self.all_rows().await?;
        self.apply_provider(&row, &rows, Some(id)).await?;
        if row.manage_dns
            && let Err(e) = self.dns_remove(&row).await
        {
            warnings.push(format!("DNS cleanup failed: {e}"));
        }
        let mut tx = self.ctx.db.begin_write().await?;
        repo::hostname_delete(&mut tx, id).await?;
        let mut ev = AuditEvent::new("hostname.delete", "hostname")
            .entity(id)
            .meta(json!({ "hostname": row.hostname, "provider": row.ingress_provider, "warnings": warnings.len() }));
        if let Some(p) = &row.project_id {
            ev = ev.project(p);
        }
        audit::write(&self.ctx, &mut tx, &AuditActor::from_actor(actor), &ev).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(DeleteOutcome { warnings })
    }

    /// Manual "Re-sync": push every provider's full config and every managed DNS record.
    pub async fn resync(&self, actor: &Actor) -> Result<ResyncReport, CoreError> {
        actor.authorize(&OP_HOSTNAMES_RESYNC, None)?;
        let _g = self.lock.lock().await;
        let rows = self.all_rows().await?;
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut done = std::collections::BTreeSet::new();
        for r in &rows {
            let scope = (r.ingress_provider.clone(), r.tunnel_id.clone());
            if done.insert(scope) {
                match self.apply_provider(r, &rows, None).await {
                    Ok(()) => {}
                    Err(e) => errors.push(format!("{}: {e}", r.ingress_provider)),
                }
            }
        }
        for r in &rows {
            let res = self.publish_row_state(r, &rows, true).await;
            match res {
                Ok(w) => warnings.extend(w),
                Err(e) => errors.push(format!("{}: {e}", r.hostname)),
            }
        }
        audit::write_standalone(
            &self.ctx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("hostname.resync", "hostname")
                .meta(json!({ "hostnames": rows.len(), "errors": errors.len() })),
        )
        .await?;
        Ok(ResyncReport {
            hostnames: rows.len(),
            errors,
            warnings,
        })
    }

    /// The Host public IP used for Traefik A/AAAA records (`host.public_ip`, `host.public_ip_v6`).
    pub async fn public_ip_set(
        &self,
        actor: &Actor,
        v4: Option<&str>,
        v6: Option<&str>,
    ) -> Result<(), CoreError> {
        actor.authorize(&OP_HOSTNAMES_PUBLIC_IP_SET, None)?;
        let v4 = v4.map(str::trim).filter(|s| !s.is_empty());
        let v6 = v6.map(str::trim).filter(|s| !s.is_empty());
        if let Some(a) = v4
            && a.parse::<std::net::Ipv4Addr>().is_err()
        {
            return Err(CoreError::validation(
                "public_ip",
                "must be an IPv4 address",
            ));
        }
        if let Some(a) = v6
            && a.parse::<std::net::Ipv6Addr>().is_err()
        {
            return Err(CoreError::validation(
                "public_ip_v6",
                "must be an IPv6 address",
            ));
        }
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        for (k, v) in [("host.public_ip", v4), ("host.public_ip_v6", v6)] {
            match v {
                Some(v) => {
                    settings::set(
                        &mut tx,
                        k,
                        &serde_json::to_string(v).unwrap_or_default(),
                        now,
                    )
                    .await?
                }
                None => settings::delete(&mut tx, k).await?,
            }
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("hostname.public_ip_set", "settings")
                .meta(json!({ "v4": v4.is_some(), "v6": v6.is_some() })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    // ---- publishing ----------------------------------------------------------------------

    async fn all_rows(&self) -> Result<Vec<repo::HostnameRow>, CoreError> {
        let mut conn = self.reader().await?;
        repo::hostname_list(&mut conn).await
    }

    /// Resolve the loopback port a row targets.
    async fn target_port(&self, r: &repo::HostnameRow) -> Result<u16, String> {
        if r.target_kind == "registry" {
            return match self.ports.registry.listen_port().await {
                Ok(Some(p)) => Ok(p),
                Ok(None) => Err("the Hosted Registry is not enabled".into()),
                Err(e) => Err(e.to_string()),
            };
        }
        match (&r.bind_address, r.host_port) {
            (Some(a), Some(p)) if a == "127.0.0.1" => {
                u16::try_from(p).map_err(|_| "invalid port".to_owned())
            }
            _ => Err("the target port is no longer declared on 127.0.0.1".into()),
        }
    }

    /// Hosts of the provider scope of `row` (same provider and tunnel), as `(hostname, port)`,
    /// leaving out disabled rows, `exclude`, and rows whose target cannot be resolved.
    async fn scope_hosts(
        &self,
        row: &repo::HostnameRow,
        rows: &[repo::HostnameRow],
        exclude: Option<&str>,
    ) -> Vec<(String, u16)> {
        let mut out = Vec::new();
        for r in rows {
            if r.ingress_provider != row.ingress_provider
                || r.tunnel_id != row.tunnel_id
                || !r.enabled
                || Some(r.id.as_str()) == exclude
            {
                continue;
            }
            if let Ok(p) = self.target_port(r).await {
                out.push((r.hostname.clone(), p));
            }
        }
        out
    }

    /// Push the full provider config for the scope of `row`.
    async fn apply_provider(
        &self,
        row: &repo::HostnameRow,
        rows: &[repo::HostnameRow],
        exclude: Option<&str>,
    ) -> Result<(), CoreError> {
        let hosts = self.scope_hosts(row, rows, exclude).await;
        match Provider::parse(&row.ingress_provider)? {
            Provider::Traefik => self.traefik_apply(&hosts).await,
            Provider::CloudflareTunnel => {
                let tid = row
                    .tunnel_id
                    .as_deref()
                    .ok_or_else(|| CoreError::new(crate::ErrorKind::Internal, "tunnel missing"))?;
                self.tunnel_apply(tid, &hosts).await
            }
        }
    }

    async fn traefik_apply(&self, hosts: &[(String, u16)]) -> Result<(), CoreError> {
        let routes: Vec<Route> = hosts
            .iter()
            .map(|(h, p)| Route {
                hostname: h.clone(),
                port: *p,
            })
            .collect();
        let content = traefik::render(
            &routes,
            &traefik::RenderSettings {
                entrypoint: self.settings.entrypoint.clone(),
                cert_resolver: self.settings.cert_resolver.clone(),
            },
        )?;
        let current = self.ports.traefik.read_dynamic()?;
        if current.as_deref() != Some(content.as_str()) {
            self.ports.traefik.write_dynamic(&content)?;
        }
        let mut tx = self.ctx.db.begin_write().await?;
        settings::set(
            &mut tx,
            TRAEFIK_HASH_KEY,
            &serde_json::to_string(&sha256_hex(&content)).unwrap_or_default(),
            self.ctx.now(),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    async fn tunnel_creds(
        &self,
        tunnel_id: &str,
    ) -> Result<(repo::TunnelRow, String, Secret), CoreError> {
        let mut conn = self.reader().await?;
        let t = repo::tunnel_get(&mut conn, tunnel_id)
            .await?
            .ok_or_else(|| CoreError::not_found("tunnel not found"))?;
        let (c, tok) = self.cloudflare.credentials(&mut conn, &t.client_id).await?;
        Ok((t, c.account_id, tok))
    }

    async fn tunnel_apply(
        &self,
        tunnel_id: &str,
        hosts: &[(String, u16)],
    ) -> Result<(), CoreError> {
        let (t, account, token) = self.tunnel_creds(tunnel_id).await?;
        let rules = build_ingress(hosts);
        self.ports
            .cloudflare
            .put_ingress(
                CfAuth {
                    account_id: &account,
                    token: &token,
                },
                &t.cf_tunnel_id,
                &rules,
            )
            .await
            .map_err(cf_err)
    }

    /// Apply the provider for one row, then DNS, verify, and record the result on the row.
    async fn publish(&self, id: &str) -> Result<PublishOutcome, CoreError> {
        let _g = self.lock.lock().await;
        let rows = self.all_rows().await?;
        let row = rows
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| CoreError::not_found("hostname not found"))?;
        let mut warnings = Vec::new();
        match self.apply_provider(&row, &rows, None).await {
            Ok(()) => match self.publish_row_state(&row, &rows, false).await {
                Ok(w) => warnings = w,
                Err(e) => warnings.push(e.to_string()),
            },
            Err(e) => {
                self.record(&row.id, "error", Some(&short(&e.to_string())), false)
                    .await?;
            }
        }
        let fresh = self.load(id).await?;
        Ok(PublishOutcome {
            hostname: fresh.into(),
            warnings,
        })
    }

    async fn record(
        &self,
        id: &str,
        status: &str,
        err: Option<&str>,
        published: bool,
    ) -> Result<(), CoreError> {
        let mut tx = self.ctx.db.begin_write().await?;
        repo::hostname_set_sync(&mut tx, id, status, err, published, self.ctx.now()).await?;
        tx.commit().await.map_err(db_err)
    }

    /// After the provider config is pushed: DNS, verification and the row status. Returns
    /// warnings; an `Err` is a failure that was already recorded on the row.
    async fn publish_row_state(
        &self,
        row: &repo::HostnameRow,
        rows: &[repo::HostnameRow],
        _resync: bool,
    ) -> Result<Vec<String>, CoreError> {
        let _ = rows;
        let mut warnings = Vec::new();
        if !row.enabled {
            self.record(&row.id, "disabled", None, false).await?;
            return Ok(warnings);
        }
        let port = match self.target_port(row).await {
            Ok(p) => p,
            Err(m) => {
                self.record(&row.id, "error", Some(&m), false).await?;
                return Err(CoreError::new(crate::ErrorKind::Conflict, m));
            }
        };
        if row.manage_dns {
            match self.dns_ensure(row).await {
                Ok(owned) => {
                    if let Some(rid) = owned {
                        let mut tx = self.ctx.db.begin_write().await?;
                        repo::hostname_set_dns_record(&mut tx, &row.id, Some(&rid)).await?;
                        tx.commit().await.map_err(db_err)?;
                    }
                }
                Err(e) => {
                    let m = format!("DNS: {}", short(&e.to_string()));
                    self.record(&row.id, "error", Some(&m), false).await?;
                    return Err(CoreError::new(crate::ErrorKind::Conflict, m));
                }
            }
        }
        match Provider::parse(&row.ingress_provider)? {
            Provider::Traefik => {
                let d = self.inspector.inspect(Daemon::Traefik).await?;
                if d.active {
                    match self
                        .ports
                        .probe
                        .await_route(80, &row.hostname, VERIFY_WAIT_SECS)
                        .await
                    {
                        Ok(404) => {
                            let journal = self
                                .ports
                                .systemd
                                .journal_tail(&d.unit, 30)
                                .await
                                .unwrap_or_default();
                            let m = format!(
                                "Traefik answered 404: the route is not loaded. Journal:\n{}",
                                short(&journal)
                            );
                            self.record(&row.id, "error", Some(&m), true).await?;
                            return Err(CoreError::new(crate::ErrorKind::Conflict, m));
                        }
                        Ok(_) => {}
                        Err(e) => warnings.push(format!("could not verify the route: {e}")),
                    }
                } else {
                    warnings
                        .push("Traefik is not running; the route will load when it starts".into());
                }
                let st = self.traefik_status_inner().await?;
                if st.watch == Some(false) {
                    warnings.push("file provider watch is off: restart Traefik to apply".into());
                }
                warnings.push(
                    "certificate issuance is asynchronous; it may stay pending for a while".into(),
                );
            }
            Provider::CloudflareTunnel => {
                let d = self.inspector.inspect(Daemon::Cloudflared).await?;
                if !d.active {
                    warnings.push("connector not connected: cloudflared is not running".into());
                } else if let Some(tid) = &row.tunnel_id
                    && let Ok((t, account, token)) = self.tunnel_creds(tid).await
                    && let Ok(api) = self
                        .ports
                        .cloudflare
                        .get_tunnel(
                            CfAuth {
                                account_id: &account,
                                token: &token,
                            },
                            &t.cf_tunnel_id,
                        )
                        .await
                    && matches!(api.status.as_str(), "inactive" | "down")
                {
                    warnings
                        .push("connector not connected: the tunnel reports no connection".into());
                }
            }
        }
        let _ = port;
        self.record(&row.id, "active", None, true).await?;
        Ok(warnings)
    }

    // ---- DNS ----------------------------------------------------------------------------

    async fn zone_token(&self, zone_id: &str) -> Result<(repo::ZoneRow, Secret), CoreError> {
        let mut conn = self.reader().await?;
        let zone = repo::zone_get(&mut conn, zone_id)
            .await?
            .ok_or_else(|| CoreError::not_found("zone not found"))?;
        let (_, token) = self
            .cloudflare
            .credentials(&mut conn, &zone.client_id)
            .await?;
        Ok((zone, token))
    }

    /// Create or update the managed records. Returns the id of a record Yanto owns (created by it
    /// or carrying its marker), to be stored as `dns_record_id`.
    async fn dns_ensure(&self, row: &repo::HostnameRow) -> Result<Option<String>, CoreError> {
        let zone_id = row
            .zone_id
            .as_deref()
            .ok_or_else(|| CoreError::validation("zone_id", "no zone is linked"))?;
        let (zone, token) = self.zone_token(zone_id).await?;
        let cf = &self.ports.cloudflare;
        let existing = cf
            .list_dns(&token, &zone.id, None, Some(&row.hostname))
            .await
            .map_err(cf_err)?;
        let wanted: Vec<(&str, String, bool)> = match Provider::parse(&row.ingress_provider)? {
            Provider::CloudflareTunnel => {
                let cft = row.cf_tunnel_id.as_deref().unwrap_or_default();
                vec![("CNAME", cname_target(cft), true)]
            }
            Provider::Traefik => {
                let mut conn = self.reader().await?;
                let (v4, v6) = CloudflareService::public_ips(&mut conn).await?;
                let mut w = Vec::new();
                if let Some(a) = v4 {
                    w.push(("A", a, false));
                }
                if let Some(a) = v6 {
                    w.push(("AAAA", a, false));
                }
                if w.is_empty() {
                    return Err(CoreError::validation(
                        "manage_dns",
                        "the Host public IP is not set",
                    ));
                }
                w
            }
        };
        let allowed: Vec<&str> = wanted.iter().map(|w| w.0).collect();
        if let Some(c) = existing
            .iter()
            .find(|r| !allowed.contains(&r.record_type.as_str()))
        {
            return Err(CoreError::conflict(format!(
                "DNS conflict: {} record {} -> {} already exists; remove it or choose another hostname",
                c.record_type, c.name, c.content
            )));
        }
        let mut owned_id = None;
        for (ty, content, proxied) in wanted {
            let input = DnsRecordInput {
                record_type: ty.to_owned(),
                name: row.hostname.clone(),
                content: content.clone(),
                ttl: 1,
                proxied,
                priority: None,
                comment: Some(MANAGED_COMMENT.to_owned()),
            };
            let found: Option<&CfDnsRecord> = existing.iter().find(|r| r.record_type == ty);
            let rec_id = match found {
                None => Some(
                    cf.create_dns(&token, &zone.id, &input)
                        .await
                        .map_err(cf_err)?
                        .id,
                ),
                Some(r) if r.content == content && r.proxied == proxied => {
                    CloudflareService::is_managed_comment(r.comment.as_deref())
                        .then(|| r.id.clone())
                }
                Some(r) => Some(
                    cf.update_dns(&token, &zone.id, &r.id, &input)
                        .await
                        .map_err(cf_err)?
                        .id,
                ),
            };
            if owned_id.is_none() {
                owned_id = rec_id;
            }
        }
        Ok(owned_id)
    }

    /// Delete the DNS records Yanto owns for this Hostname.
    async fn dns_remove(&self, row: &repo::HostnameRow) -> Result<(), CoreError> {
        let Some(zone_id) = row.zone_id.as_deref() else {
            return Ok(());
        };
        let (zone, token) = self.zone_token(zone_id).await?;
        let cf = &self.ports.cloudflare;
        if let Some(rid) = &row.dns_record_id
            && let Err(e) = cf.delete_dns(&token, &zone.id, rid).await
            && e.status != 404
        {
            return Err(cf_err(e));
        }
        if row.ingress_provider == "traefik" {
            let extra = cf
                .list_dns(&token, &zone.id, Some("AAAA"), Some(&row.hostname))
                .await
                .map_err(cf_err)?;
            for r in extra {
                if CloudflareService::is_managed_comment(r.comment.as_deref()) {
                    cf.delete_dns(&token, &zone.id, &r.id)
                        .await
                        .map_err(cf_err)?;
                }
            }
        }
        Ok(())
    }

    // ---- Traefik status ------------------------------------------------------------------

    pub async fn traefik_status(&self, actor: &Actor) -> Result<TraefikStatus, CoreError> {
        actor.authorize(&OP_TRAEFIK_STATUS, None)?;
        self.traefik_status_inner().await
    }

    async fn traefik_status_inner(&self) -> Result<TraefikStatus, CoreError> {
        let daemon = self.inspector.inspect(Daemon::Traefik).await?;
        let dir = self.ports.traefik.dir_status();
        let static_path = self.settings.traefik_static_config.clone();
        let text = self.ports.traefik.read_static(&static_path)?;
        let checklist = traefik::check_static(
            &static_path,
            text.as_deref(),
            &self.settings.traefik_dynamic_dir,
            &self.settings.entrypoint,
            &self.settings.cert_resolver,
        );
        let failed = checklist.iter().find(|c| c.status == CheckStatus::Fail);
        let not_ready_reason = if !daemon.installed {
            Some("Traefik is not installed (see the install instructions)".to_owned())
        } else if !dir.exists {
            Some(format!(
                "{} does not exist",
                self.settings.traefik_dynamic_dir.display()
            ))
        } else if !dir.writable {
            Some(format!(
                "{} is not writable",
                self.settings.traefik_dynamic_dir.display()
            ))
        } else {
            failed.map(|c| format!("{}: {}", c.name, c.detail))
        };
        let watch = checklist
            .iter()
            .find(|c| c.name == "file provider watch")
            .and_then(|c| match c.status {
                CheckStatus::Pass => Some(true),
                CheckStatus::Fail => Some(false),
                CheckStatus::Unverifiable => None,
            });
        let rows = self.all_rows().await?;
        let mut hosts = Vec::new();
        for r in rows
            .iter()
            .filter(|r| r.ingress_provider == "traefik" && r.enabled)
        {
            if let Ok(p) = self.target_port(r).await {
                hosts.push(Route {
                    hostname: r.hostname.clone(),
                    port: p,
                });
            }
        }
        let desired = traefik::render(
            &hosts,
            &traefik::RenderSettings {
                entrypoint: self.settings.entrypoint.clone(),
                cert_resolver: self.settings.cert_resolver.clone(),
            },
        )?;
        let current = self.ports.traefik.read_dynamic()?;
        let file_state = match &current {
            None if hosts.is_empty() => FileState::InSync,
            None => FileState::Missing,
            Some(c) if *c == desired => FileState::InSync,
            Some(_) => FileState::Drift,
        };
        Ok(TraefikStatus {
            daemon,
            dynamic_dir: self.settings.traefik_dynamic_dir.display().to_string(),
            dir_exists: dir.exists,
            dir_writable: dir.writable,
            checklist,
            ready: not_ready_reason.is_none(),
            not_ready_reason,
            file_state,
            watch,
            hostnames: hosts.len(),
            entrypoint: self.settings.entrypoint.clone(),
            cert_resolver: self.settings.cert_resolver.clone(),
            prerequisites: traefik::prerequisites_yaml(
                &self.settings.traefik_dynamic_dir,
                &self.settings.entrypoint,
                &self.settings.cert_resolver,
            ),
        })
    }

    // ---- diagnostics ---------------------------------------------------------------------

    /// Read-only checks per visible Hostname (04 9.2). `public` adds the opt-in `HEAD https://`.
    pub async fn diagnostics(
        &self,
        actor: &Actor,
        hostname_id: Option<&str>,
        public: bool,
    ) -> Result<Vec<HostnameDiagnostics>, CoreError> {
        actor.authorize(&OP_HOSTNAMES_DIAGNOSTICS, None)?;
        let rows = self.all_rows().await?;
        let mut out = Vec::new();
        if let Some(id) = hostname_id {
            let r = rows
                .iter()
                .find(|r| r.id == id && self.visible(actor, r))
                .ok_or_else(|| CoreError::not_found("hostname not found"))?;
            out.push(self.diagnose(r, &rows, public).await);
        } else {
            for r in rows.iter().filter(|r| self.visible(actor, r)) {
                out.push(self.diagnose(r, &rows, public).await);
            }
        }
        Ok(out)
    }

    async fn diagnose(
        &self,
        row: &repo::HostnameRow,
        rows: &[repo::HostnameRow],
        public: bool,
    ) -> HostnameDiagnostics {
        let mut checks = Vec::new();
        let port = self.target_port(row).await;
        match Provider::parse(&row.ingress_provider) {
            Ok(Provider::CloudflareTunnel) => self.diag_tunnel(row, rows, &mut checks).await,
            Ok(Provider::Traefik) => self.diag_traefik(row, &mut checks).await,
            Err(_) => {}
        }
        // Target port declared and listening; local HTTP probe.
        match &port {
            Err(m) => checks.push(DiagCheck::fail(
                "target port",
                m.clone(),
                "declare the port on 127.0.0.1 in the Project and redeploy",
            )),
            Ok(p) => {
                if self.ports.probe.tcp_connect(*p).await {
                    checks.push(DiagCheck::pass(
                        "target port",
                        format!("127.0.0.1:{p} accepts connections"),
                    ));
                    match self.ports.probe.local_http(*p, &row.hostname, 3).await {
                        Ok(s) => {
                            checks.push(DiagCheck::pass("local HTTP probe", format!("HTTP {s}")))
                        }
                        Err(e) => checks.push(DiagCheck::warn(
                            "local HTTP probe",
                            e,
                            "the service accepts TCP but did not answer HTTP",
                        )),
                    }
                } else {
                    checks.push(DiagCheck::fail(
                        "target port",
                        format!("nothing listens on 127.0.0.1:{p}"),
                        "start the Project's container (deploy it) so the port is published",
                    ));
                }
            }
        }
        self.diag_dns(row, &mut checks).await;
        if public {
            match self.ports.probe.public_head(&row.hostname).await {
                Ok(s) if s < 500 => {
                    checks.push(DiagCheck::pass("public probe", format!("HTTP {s}")))
                }
                Ok(s) => checks.push(DiagCheck::warn(
                    "public probe",
                    format!("HTTP {s}"),
                    "the origin answered with a server error",
                )),
                Err(e) => checks.push(DiagCheck::fail(
                    "public probe",
                    e,
                    "check DNS, the provider and the certificate",
                )),
            }
        }
        HostnameDiagnostics {
            hostname: row.clone().into(),
            checks,
        }
    }

    async fn diag_tunnel(
        &self,
        row: &repo::HostnameRow,
        rows: &[repo::HostnameRow],
        checks: &mut Vec<DiagCheck>,
    ) {
        let Some(tid) = row.tunnel_id.as_deref() else {
            return;
        };
        let (t, account, token) = match self.tunnel_creds(tid).await {
            Ok(v) => v,
            Err(e) => {
                checks.push(DiagCheck::fail(
                    "API token",
                    e.to_string(),
                    "re-enter the Cloudflare API token",
                ));
                return;
            }
        };
        let auth = CfAuth {
            account_id: &account,
            token: &token,
        };
        let cf = &self.ports.cloudflare;
        match cf.check_tunnels(auth).await {
            Ok(()) => checks.push(DiagCheck::pass("API token", "valid for the account")),
            Err(e) => {
                checks.push(DiagCheck::fail(
                    "API token",
                    e.to_string(),
                    "check the token and its permissions (Tunnel: Edit, DNS: Edit, Zone: Read)",
                ));
                return;
            }
        }
        match cf.get_tunnel(auth, &t.cf_tunnel_id).await {
            Ok(api) => {
                let detail = format!("tunnel {} status {}", t.name, api.status);
                if matches!(api.status.as_str(), "inactive" | "down") {
                    checks.push(DiagCheck::warn(
                        "tunnel",
                        detail,
                        "no connector is attached; run the install command on the Host",
                    ));
                } else {
                    checks.push(DiagCheck::pass("tunnel", detail));
                }
            }
            Err(e) => checks.push(DiagCheck::fail(
                "tunnel",
                e.to_string(),
                "the tunnel may have been deleted in Cloudflare",
            )),
        }
        match self.inspector.inspect(Daemon::Cloudflared).await {
            Ok(d) if d.active => checks.push(DiagCheck::pass("host cloudflared", "running")),
            Ok(d) => checks.push(DiagCheck::fail(
                "host cloudflared",
                d.state_label,
                "install and start cloudflared with the tunnel's install command",
            )),
            Err(e) => checks.push(DiagCheck::fail(
                "host cloudflared",
                e.to_string(),
                "check systemd",
            )),
        }
        let hosts: Vec<(String, u16)> = {
            let mut v = Vec::new();
            for r in rows
                .iter()
                .filter(|r| r.tunnel_id == row.tunnel_id && r.enabled)
            {
                if let Ok(p) = self.target_port(r).await {
                    v.push((r.hostname.clone(), p));
                }
            }
            v
        };
        match cf.get_ingress(auth, &t.cf_tunnel_id).await {
            Ok(remote) if drifted(&build_ingress(&hosts), &remote) => checks.push(DiagCheck::warn(
                "ingress drift",
                "the remote ingress differs from Yanto's",
                "use Re-sync; manual edits in the Cloudflare dashboard are overwritten",
            )),
            Ok(_) => checks.push(DiagCheck::pass("ingress drift", "remote ingress matches")),
            Err(e) => checks.push(DiagCheck::fail(
                "ingress drift",
                e.to_string(),
                "could not read the remote configuration",
            )),
        }
    }

    async fn diag_traefik(&self, row: &repo::HostnameRow, checks: &mut Vec<DiagCheck>) {
        match self.traefik_status_inner().await {
            Ok(st) => {
                if st.ready {
                    checks.push(DiagCheck::pass(
                        "traefik prerequisites",
                        "static configuration looks right",
                    ));
                } else {
                    checks.push(DiagCheck::fail(
                        "traefik prerequisites",
                        st.not_ready_reason.clone().unwrap_or_default(),
                        "fix the static configuration shown on the Hostnames page",
                    ));
                }
                checks.push(if st.daemon.active {
                    DiagCheck::pass("host traefik", "running")
                } else {
                    DiagCheck::fail("host traefik", st.daemon.state_label, "start Traefik")
                });
                checks.push(match st.file_state {
                    FileState::InSync => {
                        DiagCheck::pass("dynamic file", "yanto.yml matches the database")
                    }
                    FileState::Missing => {
                        DiagCheck::fail("dynamic file", "yanto.yml is missing", "use Re-sync")
                    }
                    FileState::Drift => DiagCheck::warn(
                        "dynamic file",
                        "yanto.yml differs from the database",
                        "use Re-sync; manual edits are overwritten",
                    ),
                });
                if row.enabled
                    && st.daemon.active
                    && let Ok(s) = self.ports.probe.local_http(80, &row.hostname, 3).await
                {
                    checks.push(if s == 404 {
                        DiagCheck::fail(
                            "traefik route",
                            "Traefik answers 404 for this host",
                            "the router is not loaded; check journalctl -u traefik",
                        )
                    } else {
                        DiagCheck::pass("traefik route", format!("HTTP {s}"))
                    });
                }
            }
            Err(e) => checks.push(DiagCheck::fail("traefik", e.to_string(), "check systemd")),
        }
    }

    async fn diag_dns(&self, row: &repo::HostnameRow, checks: &mut Vec<DiagCheck>) {
        let Some(zone_id) = row.zone_id.as_deref() else {
            checks.push(DiagCheck::warn(
                "dns record",
                "no Cloudflare zone is linked",
                if row.ingress_provider == "traefik" {
                    "create an A/AAAA record to the Host's public IP manually"
                } else {
                    "create a CNAME to <tunnel-id>.cfargotunnel.com (proxied) manually"
                },
            ));
            return;
        };
        let (zone, token) = match self.zone_token(zone_id).await {
            Ok(v) => v,
            Err(e) => {
                checks.push(DiagCheck::fail(
                    "dns record",
                    e.to_string(),
                    "re-link the zone",
                ));
                return;
            }
        };
        let recs = match self
            .ports
            .cloudflare
            .list_dns(&token, &zone.id, None, Some(&row.hostname))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                checks.push(DiagCheck::fail(
                    "dns record",
                    e.to_string(),
                    "check the API token",
                ));
                return;
            }
        };
        if row.ingress_provider == "cloudflare_tunnel" {
            let want = cname_target(row.cf_tunnel_id.as_deref().unwrap_or_default());
            match recs.iter().find(|r| r.record_type == "CNAME") {
                Some(r) if r.content == want && r.proxied => checks.push(DiagCheck::pass(
                    "dns record",
                    format!("CNAME -> {want} (proxied)"),
                )),
                Some(r) => checks.push(DiagCheck::fail(
                    "dns record",
                    format!("CNAME points at {} (proxied: {})", r.content, r.proxied),
                    format!("it must be {want}, proxied"),
                )),
                None => checks.push(DiagCheck::fail(
                    "dns record",
                    "no CNAME exists for this hostname",
                    "enable Manage DNS and Re-sync",
                )),
            }
        } else if recs
            .iter()
            .any(|r| matches!(r.record_type.as_str(), "A" | "AAAA"))
        {
            checks.push(DiagCheck::pass("dns record", "an A/AAAA record exists"));
        } else {
            checks.push(DiagCheck::fail(
                "dns record",
                "no A/AAAA record exists",
                "point the name at the Host's public IP",
            ));
        }
    }
}
