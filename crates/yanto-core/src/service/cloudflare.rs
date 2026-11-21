//! Cloudflare clients, zones, remote-managed tunnels and DNS records (docs/rewrite/04 9.2, 9.4).
//!
//! Cloudflare is the source of truth: local rows are the sealed API token, a cache and links. The
//! tunnel connector token is fetched at reveal time and never stored.

use serde::Serialize;
use serde_json::json;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use super::daemons::{DaemonDto, DaemonInspector};
use crate::actor::{Access, Actor, OpSpec, Scope};
use crate::crypto::new_id;
use crate::db::repo::ingress as repo;
use crate::db::repo::{db_err, settings};
use crate::error::CoreError;
use crate::ingress::cloudflare::{MANAGED_COMMENT, service_install_command};
use crate::ingress::daemon::Daemon;
use crate::ports::{CfAuth, CfDnsRecord, CfError, DnsRecordInput, IngressPorts};
use crate::secrets::Secret;

pub const OP_CLOUDFLARE_CLIENTS_LIST: OpSpec =
    OpSpec::new("cloudflare_clients_list", Access::Owner, Scope::Read);
pub const OP_CLOUDFLARE_CLIENT_VALIDATE: OpSpec =
    OpSpec::new("cloudflare_client_validate", Access::Owner, Scope::Admin);
pub const OP_CLOUDFLARE_CLIENT_CREATE: OpSpec =
    OpSpec::new("cloudflare_client_create", Access::Owner, Scope::Admin);
pub const OP_CLOUDFLARE_CLIENT_UPDATE: OpSpec =
    OpSpec::new("cloudflare_client_update", Access::Owner, Scope::Admin);
pub const OP_CLOUDFLARE_CLIENT_DELETE: OpSpec =
    OpSpec::new("cloudflare_client_delete", Access::Owner, Scope::Admin);
pub const OP_CLOUDFLARE_ZONES_LIST: OpSpec =
    OpSpec::new("cloudflare_zones_list", Access::Owner, Scope::Read);
pub const OP_CLOUDFLARE_TUNNELS_LIST: OpSpec =
    OpSpec::new("cloudflare_tunnels_list", Access::Owner, Scope::Read);
pub const OP_CLOUDFLARE_TUNNEL_CREATE: OpSpec =
    OpSpec::new("cloudflare_tunnel_create", Access::Owner, Scope::Write);
pub const OP_CLOUDFLARE_TUNNEL_ACTIVATE: OpSpec =
    OpSpec::new("cloudflare_tunnel_activate", Access::Owner, Scope::Admin);
pub const OP_CLOUDFLARE_TUNNEL_STATUS: OpSpec =
    OpSpec::new("cloudflare_tunnel_status", Access::Owner, Scope::Read);
pub const OP_CLOUDFLARE_TUNNEL_DELETE: OpSpec =
    OpSpec::new("cloudflare_tunnel_delete", Access::Owner, Scope::Admin);
/// Dashboard/CLI only (03 a.5): the connector token never travels over REST or MCP.
pub const OP_CLOUDFLARE_TUNNEL_TOKEN_REVEAL: OpSpec = OpSpec::new(
    "cloudflare_tunnel_token_reveal",
    Access::Owner,
    Scope::Admin,
)
.session_only();
pub const OP_CLOUDFLARE_DNS_LIST: OpSpec =
    OpSpec::new("cloudflare_dns_list", Access::Owner, Scope::Read);
pub const OP_CLOUDFLARE_DNS_CREATE: OpSpec =
    OpSpec::new("cloudflare_dns_create", Access::Owner, Scope::Write);
pub const OP_CLOUDFLARE_DNS_UPDATE: OpSpec =
    OpSpec::new("cloudflare_dns_update", Access::Owner, Scope::Write);
pub const OP_CLOUDFLARE_DNS_DELETE: OpSpec =
    OpSpec::new("cloudflare_dns_delete", Access::Owner, Scope::Admin);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_CLOUDFLARE_CLIENTS_LIST,
    &OP_CLOUDFLARE_CLIENT_VALIDATE,
    &OP_CLOUDFLARE_CLIENT_CREATE,
    &OP_CLOUDFLARE_CLIENT_UPDATE,
    &OP_CLOUDFLARE_CLIENT_DELETE,
    &OP_CLOUDFLARE_ZONES_LIST,
    &OP_CLOUDFLARE_TUNNELS_LIST,
    &OP_CLOUDFLARE_TUNNEL_CREATE,
    &OP_CLOUDFLARE_TUNNEL_ACTIVATE,
    &OP_CLOUDFLARE_TUNNEL_STATUS,
    &OP_CLOUDFLARE_TUNNEL_DELETE,
    &OP_CLOUDFLARE_TUNNEL_TOKEN_REVEAL,
    &OP_CLOUDFLARE_DNS_LIST,
    &OP_CLOUDFLARE_DNS_CREATE,
    &OP_CLOUDFLARE_DNS_UPDATE,
    &OP_CLOUDFLARE_DNS_DELETE,
];

/// Token permissions the client form lists (ASSUMPTION, OQ-27: verify against Cloudflare docs).
pub const TOKEN_PERMISSIONS: &[&str] = &[
    "Account: Cloudflare Tunnel: Edit",
    "Zone: DNS: Edit",
    "Zone: Zone: Read",
];

const DNS_TYPES: [&str; 6] = ["A", "AAAA", "CNAME", "TXT", "MX", "NS"];

#[derive(Debug, Clone, Serialize)]
pub struct ClientDto {
    pub id: String,
    pub name: String,
    pub account_id: String,
    pub tunnels: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ZoneDto {
    pub id: String,
    pub client_id: String,
    pub name: String,
    pub status: Option<String>,
    pub synced_at: i64,
}

impl From<repo::ZoneRow> for ZoneDto {
    fn from(z: repo::ZoneRow) -> Self {
        Self {
            id: z.id,
            client_id: z.client_id,
            name: z.name,
            status: z.status,
            synced_at: z.synced_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TunnelDto {
    pub id: String,
    pub client_id: String,
    pub cf_tunnel_id: String,
    pub name: String,
    pub active: bool,
    pub status: String,
    pub last_health_at: Option<i64>,
    pub hostnames: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TunnelCreated {
    pub tunnel: TunnelDto,
    /// Steps for the operator; the connector token is only shown by the audited reveal.
    pub instructions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TunnelStatus {
    pub tunnel: TunnelDto,
    pub api_status: String,
    pub cloudflared: DaemonDto,
    /// What to do when the tunnel has no connector.
    pub hint: Option<String>,
}

/// Result of the audited reveal; both values are secrets (redacting `Debug`).
#[derive(Debug)]
pub struct TokenReveal {
    pub tunnel_name: String,
    pub token: Secret,
    pub install_command: Secret,
}

#[derive(Debug, Clone, Serialize)]
pub struct DnsDto {
    pub id: String,
    pub zone_id: String,
    pub record_type: String,
    pub name: String,
    pub content: String,
    pub ttl: i64,
    pub proxied: bool,
    pub priority: Option<i64>,
    pub comment: Option<String>,
    /// Bound to a Hostname: edit and delete go through the Hostname.
    pub managed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DnsInput {
    pub record_type: String,
    pub name: String,
    pub content: String,
    /// `1` means automatic.
    pub ttl: Option<i64>,
    pub proxied: bool,
    pub priority: Option<i64>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidateReport {
    pub zones: Vec<String>,
    pub permissions_needed: Vec<&'static str>,
}

/// Map an API failure to a caller-safe error that names the HTTP status and Cloudflare's message.
pub fn cf_err(e: CfError) -> CoreError {
    CoreError::new(crate::ErrorKind::Unavailable, e.to_string())
}

fn cf_token_err(e: CfError) -> CoreError {
    if matches!(e.status, 400 | 401 | 403) {
        CoreError::validation(
            "api_token",
            format!(
                "Cloudflare rejected the API token (HTTP {}): {}",
                e.status, e.message
            ),
        )
    } else {
        cf_err(e)
    }
}

fn to_dto(r: CfDnsRecord, zone: &str, managed: bool) -> DnsDto {
    DnsDto {
        id: r.id,
        zone_id: zone.to_owned(),
        record_type: r.record_type,
        name: r.name,
        content: r.content,
        ttl: r.ttl,
        proxied: r.proxied,
        priority: r.priority,
        comment: r.comment,
        managed,
    }
}

fn cached_to_dto(r: repo::DnsRow, managed: bool) -> DnsDto {
    DnsDto {
        id: r.id,
        zone_id: r.zone_id,
        record_type: r.record_type,
        name: r.name,
        content: r.content,
        ttl: r.ttl,
        proxied: r.proxied,
        priority: r.priority,
        comment: r.comment,
        managed,
    }
}

#[derive(Clone)]
pub struct CloudflareService {
    ctx: Ctx,
    ports: IngressPorts,
    inspector: DaemonInspector,
}

impl std::fmt::Debug for CloudflareService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudflareService").finish_non_exhaustive()
    }
}

impl CloudflareService {
    pub(super) fn new(ctx: Ctx, ports: IngressPorts, inspector: DaemonInspector) -> Self {
        Self {
            ctx,
            ports,
            inspector,
        }
    }

    async fn reader(&self) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, CoreError> {
        self.ctx.db.reader().acquire().await.map_err(db_err)
    }

    /// The client row and its decrypted API token.
    pub(crate) async fn credentials(
        &self,
        conn: &mut sqlx::SqliteConnection,
        client_id: &str,
    ) -> Result<(repo::ClientRow, Secret), CoreError> {
        let row = repo::client_get(conn, client_id)
            .await?
            .ok_or_else(|| CoreError::not_found("Cloudflare client not found"))?;
        let token = self.ctx.secrets.open(
            "cloudflare_clients",
            "api_token_enc",
            &row.id,
            &row.api_token_enc,
        )?;
        Ok((row, token))
    }

    fn tunnel_dto(t: repo::TunnelRow, hostnames: i64) -> TunnelDto {
        TunnelDto {
            id: t.id,
            client_id: t.client_id,
            cf_tunnel_id: t.cf_tunnel_id,
            name: t.name,
            active: t.active,
            status: t.status,
            last_health_at: t.last_health_at,
            hostnames,
            created_at: t.created_at,
        }
    }

    async fn tunnel_dto_for(
        &self,
        conn: &mut sqlx::SqliteConnection,
        t: repo::TunnelRow,
    ) -> Result<TunnelDto, CoreError> {
        let n = repo::tunnel_hostname_count(conn, &t.id).await?;
        Ok(Self::tunnel_dto(t, n))
    }

    // ---- clients -------------------------------------------------------------------------

    pub async fn clients_list(&self, actor: &Actor) -> Result<Vec<ClientDto>, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_CLIENTS_LIST, None)?;
        let mut conn = self.reader().await?;
        let mut out = Vec::new();
        for c in repo::client_list(&mut conn).await? {
            let tunnels = repo::client_tunnel_count(&mut conn, &c.id).await?;
            out.push(ClientDto {
                id: c.id,
                name: c.name,
                account_id: c.account_id,
                tunnels,
                created_at: c.created_at,
                updated_at: c.updated_at,
            });
        }
        Ok(out)
    }

    fn check_client_fields(name: &str, account_id: &str) -> Result<(), CoreError> {
        if name.trim().is_empty() || name.len() > 64 {
            return Err(CoreError::validation(
                "name",
                "name must be 1-64 characters",
            ));
        }
        let ok = account_id.len() == 32 && account_id.bytes().all(|c| c.is_ascii_hexdigit());
        if !ok {
            return Err(CoreError::validation(
                "account_id",
                "account id must be 32 hexadecimal characters",
            ));
        }
        Ok(())
    }

    /// Check the token against the account without storing anything (the token is write-only).
    async fn probe_token(
        &self,
        account_id: &str,
        token: &Secret,
    ) -> Result<Vec<crate::ports::CfZone>, CoreError> {
        let auth = CfAuth { account_id, token };
        self.ports
            .cloudflare
            .check_tunnels(auth)
            .await
            .map_err(cf_token_err)?;
        self.ports
            .cloudflare
            .list_zones(auth)
            .await
            .map_err(cf_token_err)
    }

    pub async fn client_validate(
        &self,
        actor: &Actor,
        account_id: &str,
        token: &Secret,
    ) -> Result<ValidateReport, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_CLIENT_VALIDATE, None)?;
        Self::check_client_fields("validate", account_id)?;
        let zones = self.probe_token(account_id, token).await?;
        Ok(ValidateReport {
            zones: zones.into_iter().map(|z| z.name).collect(),
            permissions_needed: TOKEN_PERMISSIONS.to_vec(),
        })
    }

    pub async fn client_create(
        &self,
        actor: &Actor,
        name: &str,
        account_id: &str,
        token: &Secret,
    ) -> Result<ClientDto, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_CLIENT_CREATE, None)?;
        let name = name.trim();
        Self::check_client_fields(name, account_id)?;
        let zones = self.probe_token(account_id, token).await?;
        let id = new_id("cfc");
        let enc = self
            .ctx
            .secrets
            .seal("cloudflare_clients", "api_token_enc", &id, token)?;
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        repo::client_insert(
            &mut tx,
            &repo::ClientRow {
                id: id.clone(),
                name: name.to_owned(),
                account_id: account_id.to_owned(),
                api_token_enc: enc,
                created_at: now,
                updated_at: now,
            },
        )
        .await?;
        let z: Vec<_> = zones
            .into_iter()
            .map(|z| (z.id, z.name, z.status))
            .collect();
        repo::zones_replace(&mut tx, &id, &z, now).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cloudflare_client.create", "cloudflare_client")
                .entity(&id)
                .meta(json!({ "name": name, "account_id": account_id, "zones": z.len() })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(ClientDto {
            id,
            name: name.to_owned(),
            account_id: account_id.to_owned(),
            tunnels: 0,
            created_at: now,
            updated_at: now,
        })
    }

    /// Rename and/or replace the API token (the new token is validated first).
    pub async fn client_update(
        &self,
        actor: &Actor,
        id: &str,
        name: Option<&str>,
        token: Option<&Secret>,
    ) -> Result<ClientDto, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_CLIENT_UPDATE, None)?;
        let existing = {
            let mut conn = self.reader().await?;
            repo::client_get(&mut conn, id)
                .await?
                .ok_or_else(|| CoreError::not_found("Cloudflare client not found"))?
        };
        let new_name = name.map(str::trim).unwrap_or(&existing.name).to_owned();
        Self::check_client_fields(&new_name, &existing.account_id)?;
        let enc = match token {
            Some(t) => {
                self.probe_token(&existing.account_id, t).await?;
                Some(
                    self.ctx
                        .secrets
                        .seal("cloudflare_clients", "api_token_enc", id, t)?,
                )
            }
            None => None,
        };
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        repo::client_update(&mut tx, id, &new_name, enc.as_deref(), now).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cloudflare_client.update", "cloudflare_client")
                .entity(id)
                .meta(json!({ "name": new_name, "token_replaced": token.is_some() })),
        )
        .await?;
        let tunnels = repo::client_tunnel_count(&mut tx, id).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(ClientDto {
            id: id.to_owned(),
            name: new_name,
            account_id: existing.account_id,
            tunnels,
            created_at: existing.created_at,
            updated_at: now,
        })
    }

    pub async fn client_delete(&self, actor: &Actor, id: &str) -> Result<(), CoreError> {
        actor.authorize(&OP_CLOUDFLARE_CLIENT_DELETE, None)?;
        let mut tx = self.ctx.db.begin_write().await?;
        let row = repo::client_get(&mut tx, id)
            .await?
            .ok_or_else(|| CoreError::not_found("Cloudflare client not found"))?;
        let n = repo::client_tunnel_count(&mut tx, id).await?;
        if n > 0 {
            return Err(CoreError::conflict(format!(
                "{n} tunnel(s) use this client; delete them first"
            )));
        }
        let used: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM hostnames WHERE zone_id IN (SELECT id FROM cloudflare_zones WHERE client_id = ?)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        if used > 0 {
            return Err(CoreError::conflict(format!(
                "{used} Hostname(s) manage DNS in this client's zones; change them first"
            )));
        }
        repo::client_delete(&mut tx, id).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cloudflare_client.delete", "cloudflare_client")
                .entity(id)
                .meta(json!({ "name": row.name })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    // ---- zones ---------------------------------------------------------------------------

    pub async fn zones_list(
        &self,
        actor: &Actor,
        client_id: &str,
        refresh: bool,
    ) -> Result<Vec<ZoneDto>, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_ZONES_LIST, None)?;
        self.zones_for(client_id, refresh).await
    }

    pub(crate) async fn zones_for(
        &self,
        client_id: &str,
        refresh: bool,
    ) -> Result<Vec<ZoneDto>, CoreError> {
        let (client, token) = {
            let mut conn = self.reader().await?;
            let c = self.credentials(&mut conn, client_id).await?;
            let cached = repo::zones_for_client(&mut conn, client_id).await?;
            if !refresh && !cached.is_empty() {
                return Ok(cached.into_iter().map(Into::into).collect());
            }
            c
        };
        let zones = self
            .ports
            .cloudflare
            .list_zones(CfAuth {
                account_id: &client.account_id,
                token: &token,
            })
            .await
            .map_err(cf_err)?;
        let z: Vec<_> = zones
            .into_iter()
            .map(|z| (z.id, z.name, z.status))
            .collect();
        let mut tx = self.ctx.db.begin_write().await?;
        repo::zones_replace(&mut tx, client_id, &z, self.ctx.now()).await?;
        let out = repo::zones_for_client(&mut tx, client_id).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(out.into_iter().map(Into::into).collect())
    }

    // ---- tunnels -------------------------------------------------------------------------

    pub async fn tunnels_list(&self, actor: &Actor) -> Result<Vec<TunnelDto>, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_TUNNELS_LIST, None)?;
        let mut conn = self.reader().await?;
        let mut out = Vec::new();
        for t in repo::tunnel_list(&mut conn).await? {
            out.push(self.tunnel_dto_for(&mut conn, t).await?);
        }
        Ok(out)
    }

    /// Create a remote-managed tunnel (`config_src = cloudflare`). The first tunnel becomes the
    /// active one. The response never carries the connector token.
    pub async fn tunnel_create(
        &self,
        actor: &Actor,
        client_id: &str,
        name: &str,
    ) -> Result<TunnelCreated, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_TUNNEL_CREATE, None)?;
        let name = name.trim();
        let name_ok = !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'));
        if !name_ok {
            return Err(CoreError::validation(
                "name",
                "tunnel name may only contain letters, digits, - _ .",
            ));
        }
        let (client, token) = {
            let mut conn = self.reader().await?;
            self.credentials(&mut conn, client_id).await?
        };
        let auth = CfAuth {
            account_id: &client.account_id,
            token: &token,
        };
        let created = self
            .ports
            .cloudflare
            .create_tunnel(auth, name)
            .await
            .map_err(cf_err)?;
        let now = self.ctx.now();
        let id = new_id("cft");
        let res: Result<repo::TunnelRow, CoreError> = async {
            let mut tx = self.ctx.db.begin_write().await?;
            let first = repo::tunnel_active(&mut tx).await?.is_none();
            let row = repo::TunnelRow {
                id: id.clone(),
                client_id: client_id.to_owned(),
                cf_tunnel_id: created.id.clone(),
                name: name.to_owned(),
                active: first,
                status: "unknown".into(),
                last_health_at: None,
                created_at: now,
                updated_at: now,
            };
            repo::tunnel_insert(&mut tx, &row).await?;
            audit::write(
                &self.ctx,
                &mut tx,
                &AuditActor::from_actor(actor),
                &AuditEvent::new("cloudflare_tunnel.create", "cloudflare_tunnel")
                    .entity(&id)
                    .meta(json!({ "name": name, "client_id": client_id, "active": first })),
            )
            .await?;
            tx.commit().await.map_err(db_err)?;
            Ok(row)
        }
        .await;
        let row = match res {
            Ok(r) => r,
            Err(e) => {
                // Do not leave an orphan tunnel behind in the account.
                let _ = self.ports.cloudflare.delete_tunnel(auth, &created.id).await;
                return Err(e);
            }
        };
        Ok(TunnelCreated {
            tunnel: Self::tunnel_dto(row, 0),
            instructions: vec![
                "Install cloudflared on the Host if it is not installed.".into(),
                "Use \"Reveal install command\" to show `cloudflared service install <TOKEN>` once, and run it as root on the Host.".into(),
                "Yanto never runs that command and never stores the token.".into(),
            ],
        })
    }

    pub async fn tunnel_activate(&self, actor: &Actor, id: &str) -> Result<(), CoreError> {
        actor.authorize(&OP_CLOUDFLARE_TUNNEL_ACTIVATE, None)?;
        let mut tx = self.ctx.db.begin_write().await?;
        let t = repo::tunnel_get(&mut tx, id)
            .await?
            .ok_or_else(|| CoreError::not_found("tunnel not found"))?;
        repo::tunnel_activate(&mut tx, id, self.ctx.now()).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cloudflare_tunnel.activate", "cloudflare_tunnel")
                .entity(id)
                .meta(json!({ "name": t.name })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    /// API status of the tunnel plus the state of the Host `cloudflared`.
    pub async fn tunnel_status(&self, actor: &Actor, id: &str) -> Result<TunnelStatus, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_TUNNEL_STATUS, None)?;
        let (tunnel, client, token) = {
            let mut conn = self.reader().await?;
            let t = repo::tunnel_get(&mut conn, id)
                .await?
                .ok_or_else(|| CoreError::not_found("tunnel not found"))?;
            let (c, tok) = self.credentials(&mut conn, &t.client_id).await?;
            (t, c, tok)
        };
        let api = self
            .ports
            .cloudflare
            .get_tunnel(
                CfAuth {
                    account_id: &client.account_id,
                    token: &token,
                },
                &tunnel.cf_tunnel_id,
            )
            .await
            .map_err(cf_err)?;
        let mut tx = self.ctx.db.begin_write().await?;
        repo::tunnel_set_status(&mut tx, id, &api.status, self.ctx.now()).await?;
        let fresh = repo::tunnel_get(&mut tx, id).await?.unwrap_or(tunnel);
        let dto = self.tunnel_dto_for(&mut tx, fresh).await?;
        tx.commit().await.map_err(db_err)?;
        let cloudflared = self.inspector.inspect(Daemon::Cloudflared).await?;
        let hint = if !cloudflared.installed {
            Some("cloudflared is not installed on the Host; see the install instructions".into())
        } else if !cloudflared.active {
            Some(
                "cloudflared is installed but not running; start it or run the install command"
                    .into(),
            )
        } else if matches!(api.status.as_str(), "inactive" | "down") {
            Some("the tunnel has no connector: cloudflared may be running with another tunnel's token".into())
        } else {
            None
        };
        Ok(TunnelStatus {
            tunnel: dto,
            api_status: api.status,
            cloudflared,
            hint,
        })
    }

    pub async fn tunnel_delete(
        &self,
        actor: &Actor,
        id: &str,
        force: bool,
    ) -> Result<(), CoreError> {
        actor.authorize(&OP_CLOUDFLARE_TUNNEL_DELETE, None)?;
        let (tunnel, client, token, n) = {
            let mut conn = self.reader().await?;
            let t = repo::tunnel_get(&mut conn, id)
                .await?
                .ok_or_else(|| CoreError::not_found("tunnel not found"))?;
            let (c, tok) = self.credentials(&mut conn, &t.client_id).await?;
            let n = repo::tunnel_hostname_count(&mut conn, id).await?;
            (t, c, tok, n)
        };
        if n > 0 {
            return Err(CoreError::conflict(format!(
                "{n} Hostname(s) use this tunnel; delete them first"
            )));
        }
        if tunnel.active && !force {
            let d = self.inspector.inspect(Daemon::Cloudflared).await?;
            if d.active {
                return Err(CoreError::conflict(
                    "stop the Host cloudflared before deleting the active tunnel (or pass force)",
                ));
            }
        }
        self.ports
            .cloudflare
            .delete_tunnel(
                CfAuth {
                    account_id: &client.account_id,
                    token: &token,
                },
                &tunnel.cf_tunnel_id,
            )
            .await
            .map_err(cf_err)?;
        let mut tx = self.ctx.db.begin_write().await?;
        repo::tunnel_delete(&mut tx, id).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cloudflare_tunnel.delete", "cloudflare_tunnel")
                .entity(id)
                .meta(json!({ "name": tunnel.name, "force": force })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    /// Fetch the connector token from the API and build the install command. Dashboard only,
    /// Owner only, audited; the token is not stored and not part of the audit row.
    pub async fn tunnel_token_reveal(
        &self,
        actor: &Actor,
        id: &str,
    ) -> Result<TokenReveal, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_TUNNEL_TOKEN_REVEAL, None)?;
        let (tunnel, client, token) = {
            let mut conn = self.reader().await?;
            let t = repo::tunnel_get(&mut conn, id)
                .await?
                .ok_or_else(|| CoreError::not_found("tunnel not found"))?;
            let (c, tok) = self.credentials(&mut conn, &t.client_id).await?;
            (t, c, tok)
        };
        let connector = self
            .ports
            .cloudflare
            .tunnel_token(
                CfAuth {
                    account_id: &client.account_id,
                    token: &token,
                },
                &tunnel.cf_tunnel_id,
            )
            .await
            .map_err(cf_err)?;
        audit::write_standalone(
            &self.ctx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cloudflare_tunnel.token_reveal", "cloudflare_tunnel")
                .entity(id)
                .meta(json!({ "name": tunnel.name })),
        )
        .await?;
        let cmd = Secret::new(service_install_command(connector.expose()));
        Ok(TokenReveal {
            tunnel_name: tunnel.name,
            token: connector,
            install_command: cmd,
        })
    }

    // ---- DNS -----------------------------------------------------------------------------

    /// Credentials for a zone, checking the zone belongs to `client_id`.
    async fn zone_access(
        &self,
        client_id: &str,
        zone_id: &str,
    ) -> Result<(repo::ZoneRow, Secret), CoreError> {
        let mut conn = self.reader().await?;
        let (_, token) = self.credentials(&mut conn, client_id).await?;
        let zone = repo::zone_get(&mut conn, zone_id).await?;
        drop(conn);
        match zone {
            Some(z) if z.client_id == client_id => Ok((z, token)),
            Some(_) => Err(CoreError::not_found("zone not found")),
            None => {
                // Not cached yet: refresh the client's zones once.
                self.zones_for(client_id, true).await?;
                let mut conn = self.reader().await?;
                match repo::zone_get(&mut conn, zone_id).await? {
                    Some(z) if z.client_id == client_id => Ok((z, token)),
                    _ => Err(CoreError::not_found("zone not found")),
                }
            }
        }
    }

    async fn managed_ids(
        &self,
        conn: &mut sqlx::SqliteConnection,
    ) -> Result<Vec<String>, CoreError> {
        sqlx::query_scalar("SELECT dns_record_id FROM hostnames WHERE dns_record_id IS NOT NULL")
            .fetch_all(conn)
            .await
            .map_err(db_err)
    }

    pub async fn dns_list(
        &self,
        actor: &Actor,
        client_id: &str,
        zone_id: &str,
        refresh: bool,
    ) -> Result<Vec<DnsDto>, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_DNS_LIST, None)?;
        let (zone, token) = self.zone_access(client_id, zone_id).await?;
        if refresh || {
            let mut conn = self.reader().await?;
            repo::dns_for_zone(&mut conn, zone_id).await?.is_empty()
        } {
            let recs = self
                .ports
                .cloudflare
                .list_dns(&token, &zone.id, None, None)
                .await
                .map_err(cf_err)?;
            let now = self.ctx.now();
            let mut tx = self.ctx.db.begin_write().await?;
            repo::dns_clear_zone(&mut tx, zone_id).await?;
            for r in recs {
                repo::dns_upsert(
                    &mut tx,
                    &repo::DnsRow {
                        id: r.id,
                        zone_id: zone_id.to_owned(),
                        record_type: r.record_type,
                        name: r.name,
                        content: r.content,
                        ttl: r.ttl,
                        proxied: r.proxied,
                        priority: r.priority,
                        comment: r.comment,
                        synced_at: now,
                    },
                )
                .await?;
            }
            tx.commit().await.map_err(db_err)?;
        }
        let mut conn = self.reader().await?;
        let managed = self.managed_ids(&mut conn).await?;
        Ok(repo::dns_for_zone(&mut conn, zone_id)
            .await?
            .into_iter()
            .map(|r| {
                let m = managed.contains(&r.id);
                cached_to_dto(r, m)
            })
            .collect())
    }

    fn validate_dns(i: &DnsInput) -> Result<DnsRecordInput, CoreError> {
        let ty = i.record_type.trim().to_ascii_uppercase();
        if !DNS_TYPES.contains(&ty.as_str()) {
            return Err(CoreError::validation(
                "type",
                "type must be one of A, AAAA, CNAME, TXT, MX, NS",
            ));
        }
        let name = i.name.trim().to_ascii_lowercase();
        if name.is_empty() || name.len() > 253 || name.chars().any(|c| c.is_whitespace()) {
            return Err(CoreError::validation(
                "name",
                "name must be 1-253 characters",
            ));
        }
        let content = i.content.trim().to_owned();
        if content.is_empty() || content.len() > 2048 {
            return Err(CoreError::validation("content", "content is required"));
        }
        match ty.as_str() {
            "A" if content.parse::<std::net::Ipv4Addr>().is_err() => {
                return Err(CoreError::validation("content", "A needs an IPv4 address"));
            }
            "AAAA" if content.parse::<std::net::Ipv6Addr>().is_err() => {
                return Err(CoreError::validation(
                    "content",
                    "AAAA needs an IPv6 address",
                ));
            }
            _ => {}
        }
        let ttl = i.ttl.unwrap_or(1);
        if ttl != 1 && !(60..=86_400).contains(&ttl) {
            return Err(CoreError::validation(
                "ttl",
                "ttl must be 1 (auto) or 60-86400",
            ));
        }
        let proxy_ok = matches!(ty.as_str(), "A" | "AAAA" | "CNAME");
        if i.proxied && !proxy_ok {
            return Err(CoreError::validation(
                "proxied",
                "only A, AAAA and CNAME can be proxied",
            ));
        }
        let priority = if ty == "MX" {
            let p = i.priority.unwrap_or(10);
            if !(0..=65_535).contains(&p) {
                return Err(CoreError::validation(
                    "priority",
                    "priority must be 0-65535",
                ));
            }
            Some(p)
        } else {
            None
        };
        Ok(DnsRecordInput {
            record_type: ty,
            name,
            content,
            ttl,
            proxied: i.proxied,
            priority,
            comment: i.comment.clone().filter(|c| !c.is_empty()),
        })
    }

    async fn cache_one(&self, zone_id: &str, r: &CfDnsRecord) -> Result<(), CoreError> {
        let mut tx = self.ctx.db.begin_write().await?;
        repo::dns_upsert(
            &mut tx,
            &repo::DnsRow {
                id: r.id.clone(),
                zone_id: zone_id.to_owned(),
                record_type: r.record_type.clone(),
                name: r.name.clone(),
                content: r.content.clone(),
                ttl: r.ttl,
                proxied: r.proxied,
                priority: r.priority,
                comment: r.comment.clone(),
                synced_at: self.ctx.now(),
            },
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    pub async fn dns_create(
        &self,
        actor: &Actor,
        client_id: &str,
        zone_id: &str,
        input: &DnsInput,
    ) -> Result<DnsDto, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_DNS_CREATE, None)?;
        let rec = Self::validate_dns(input)?;
        let (zone, token) = self.zone_access(client_id, zone_id).await?;
        let created = self
            .ports
            .cloudflare
            .create_dns(&token, &zone.id, &rec)
            .await
            .map_err(cf_err)?;
        self.cache_one(zone_id, &created).await?;
        audit::write_standalone(
            &self.ctx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cloudflare_dns.create", "cloudflare_dns")
                .entity(&created.id)
                .meta(json!({ "zone": zone.name, "type": rec.record_type, "name": rec.name, "content": rec.content })),
        )
        .await?;
        Ok(to_dto(created, zone_id, false))
    }

    pub async fn dns_update(
        &self,
        actor: &Actor,
        client_id: &str,
        zone_id: &str,
        record_id: &str,
        input: &DnsInput,
    ) -> Result<DnsDto, CoreError> {
        actor.authorize(&OP_CLOUDFLARE_DNS_UPDATE, None)?;
        let rec = Self::validate_dns(input)?;
        let (zone, token) = self.zone_access(client_id, zone_id).await?;
        {
            let mut conn = self.reader().await?;
            if self
                .managed_ids(&mut conn)
                .await?
                .iter()
                .any(|m| m == record_id)
            {
                return Err(CoreError::conflict(
                    "this record belongs to a Hostname; change it there",
                ));
            }
        }
        let updated = self
            .ports
            .cloudflare
            .update_dns(&token, &zone.id, record_id, &rec)
            .await
            .map_err(cf_err)?;
        self.cache_one(zone_id, &updated).await?;
        audit::write_standalone(
            &self.ctx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cloudflare_dns.update", "cloudflare_dns")
                .entity(record_id)
                .meta(json!({ "zone": zone.name, "type": rec.record_type, "name": rec.name, "content": rec.content })),
        )
        .await?;
        Ok(to_dto(updated, zone_id, false))
    }

    pub async fn dns_delete(
        &self,
        actor: &Actor,
        client_id: &str,
        zone_id: &str,
        record_id: &str,
    ) -> Result<(), CoreError> {
        actor.authorize(&OP_CLOUDFLARE_DNS_DELETE, None)?;
        let (zone, token) = self.zone_access(client_id, zone_id).await?;
        {
            let mut conn = self.reader().await?;
            if self
                .managed_ids(&mut conn)
                .await?
                .iter()
                .any(|m| m == record_id)
            {
                return Err(CoreError::conflict(
                    "this record belongs to a Hostname; delete the Hostname instead",
                ));
            }
        }
        self.ports
            .cloudflare
            .delete_dns(&token, &zone.id, record_id)
            .await
            .map_err(cf_err)?;
        let mut tx = self.ctx.db.begin_write().await?;
        repo::dns_delete(&mut tx, record_id).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cloudflare_dns.delete", "cloudflare_dns")
                .entity(record_id)
                .meta(json!({ "zone": zone.name })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    /// Whether a record carries Yanto's ownership marker.
    pub(crate) fn is_managed_comment(c: Option<&str>) -> bool {
        c == Some(MANAGED_COMMENT)
    }

    /// Public: settings value `host.public_ip` and `host.public_ip_v6`.
    pub(crate) async fn public_ips(
        conn: &mut sqlx::SqliteConnection,
    ) -> Result<(Option<String>, Option<String>), CoreError> {
        let get = |v: Option<String>| -> Option<String> {
            v.and_then(|j| serde_json::from_str::<String>(&j).ok())
                .filter(|s| !s.is_empty())
        };
        let v4 = get(settings::get(conn, "host.public_ip").await?);
        let v6 = get(settings::get(conn, "host.public_ip_v6").await?);
        Ok((v4, v6))
    }
}
