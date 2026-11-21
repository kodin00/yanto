//! Cloudflare clients, zones, tunnels, DNS cache, Hostnames and the FRP settings singleton.

use sqlx::{FromRow, SqliteConnection};

use super::{db_err, is_unique_violation};
use crate::error::CoreError;

// ---- clients ------------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct ClientRow {
    pub id: String,
    pub name: String,
    pub account_id: String,
    pub api_token_enc: String,
    pub created_at: i64,
    pub updated_at: i64,
}

const CLIENT: &str =
    "SELECT id, name, account_id, api_token_enc, created_at, updated_at FROM cloudflare_clients";

fn client_dup(e: sqlx::Error) -> CoreError {
    if is_unique_violation(&e) {
        CoreError::conflict("a Cloudflare client with this name or account id already exists")
    } else {
        db_err(e)
    }
}

pub async fn client_insert(conn: &mut SqliteConnection, c: &ClientRow) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO cloudflare_clients (id, name, account_id, api_token_enc, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&c.id)
    .bind(&c.name)
    .bind(&c.account_id)
    .bind(&c.api_token_enc)
    .bind(c.created_at)
    .bind(c.updated_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(client_dup)
}

pub async fn client_get(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<ClientRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{CLIENT} WHERE id = ?")))
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn client_list(conn: &mut SqliteConnection) -> Result<Vec<ClientRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{CLIENT} ORDER BY name")))
        .fetch_all(conn)
        .await
        .map_err(db_err)
}

pub async fn client_update(
    conn: &mut SqliteConnection,
    id: &str,
    name: &str,
    token_enc: Option<&str>,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE cloudflare_clients SET name = ?, api_token_enc = COALESCE(?, api_token_enc),
            updated_at = ? WHERE id = ?",
    )
    .bind(name)
    .bind(token_enc)
    .bind(now)
    .bind(id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(client_dup)
}

pub async fn client_delete(conn: &mut SqliteConnection, id: &str) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM cloudflare_clients WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn client_tunnel_count(conn: &mut SqliteConnection, id: &str) -> Result<i64, CoreError> {
    sqlx::query_scalar("SELECT COUNT(*) FROM cloudflare_tunnels WHERE client_id = ?")
        .bind(id)
        .fetch_one(conn)
        .await
        .map_err(db_err)
}

// ---- zones --------------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct ZoneRow {
    pub id: String,
    pub client_id: String,
    pub name: String,
    pub status: Option<String>,
    pub synced_at: i64,
}

pub async fn zones_for_client(
    conn: &mut SqliteConnection,
    client_id: &str,
) -> Result<Vec<ZoneRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, client_id, name, status, synced_at FROM cloudflare_zones
         WHERE client_id = ? ORDER BY name",
    )
    .bind(client_id)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn zone_get(conn: &mut SqliteConnection, id: &str) -> Result<Option<ZoneRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, client_id, name, status, synced_at FROM cloudflare_zones WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

/// Upsert the zones the API returned and drop cached zones that no longer exist. Zones used by a
/// Hostname stay (the FK is `SET NULL`, so deleting would silently turn off managed DNS).
pub async fn zones_replace(
    conn: &mut SqliteConnection,
    client_id: &str,
    zones: &[(String, String, Option<String>)],
    now: i64,
) -> Result<(), CoreError> {
    for (id, name, status) in zones {
        sqlx::query(
            "INSERT INTO cloudflare_zones (id, client_id, name, status, synced_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, status = excluded.status,
                synced_at = excluded.synced_at, client_id = excluded.client_id",
        )
        .bind(id)
        .bind(client_id)
        .bind(name)
        .bind(status)
        .bind(now)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    }
    sqlx::query(
        "DELETE FROM cloudflare_zones WHERE client_id = ? AND synced_at < ?
            AND id NOT IN (SELECT zone_id FROM hostnames WHERE zone_id IS NOT NULL)",
    )
    .bind(client_id)
    .bind(now)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

// ---- tunnels ------------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct TunnelRow {
    pub id: String,
    pub client_id: String,
    pub cf_tunnel_id: String,
    pub name: String,
    pub active: bool,
    pub status: String,
    pub last_health_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

const TUNNEL: &str = "SELECT id, client_id, cf_tunnel_id, name, active, status, last_health_at,
    created_at, updated_at FROM cloudflare_tunnels";

pub async fn tunnel_insert(conn: &mut SqliteConnection, t: &TunnelRow) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO cloudflare_tunnels (id, client_id, cf_tunnel_id, name, active, status,
            last_health_at, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&t.id)
    .bind(&t.client_id)
    .bind(&t.cf_tunnel_id)
    .bind(&t.name)
    .bind(t.active)
    .bind(&t.status)
    .bind(t.last_health_at)
    .bind(t.created_at)
    .bind(t.updated_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn tunnel_get(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<TunnelRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{TUNNEL} WHERE id = ?")))
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn tunnel_list(conn: &mut SqliteConnection) -> Result<Vec<TunnelRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{TUNNEL} ORDER BY active DESC, created_at"
    )))
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn tunnel_active(conn: &mut SqliteConnection) -> Result<Option<TunnelRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{TUNNEL} WHERE active = 1")))
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

/// Make `id` the only active tunnel.
pub async fn tunnel_activate(
    conn: &mut SqliteConnection,
    id: &str,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE cloudflare_tunnels SET active = 0, updated_at = ? WHERE active = 1")
        .bind(now)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    sqlx::query("UPDATE cloudflare_tunnels SET active = 1, updated_at = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn tunnel_set_status(
    conn: &mut SqliteConnection,
    id: &str,
    status: &str,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE cloudflare_tunnels SET status = ?, last_health_at = ? WHERE id = ?")
        .bind(status)
        .bind(now)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn tunnel_delete(conn: &mut SqliteConnection, id: &str) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM cloudflare_tunnels WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn tunnel_hostname_count(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<i64, CoreError> {
    sqlx::query_scalar("SELECT COUNT(*) FROM hostnames WHERE tunnel_id = ?")
        .bind(id)
        .fetch_one(conn)
        .await
        .map_err(db_err)
}

// ---- DNS cache ----------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct DnsRow {
    pub id: String,
    pub zone_id: String,
    #[sqlx(rename = "type")]
    pub record_type: String,
    pub name: String,
    pub content: String,
    pub ttl: i64,
    pub proxied: bool,
    pub priority: Option<i64>,
    pub comment: Option<String>,
    pub synced_at: i64,
}

pub async fn dns_for_zone(
    conn: &mut SqliteConnection,
    zone_id: &str,
) -> Result<Vec<DnsRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, zone_id, type, name, content, ttl, proxied, priority, comment, synced_at
         FROM cloudflare_dns_records WHERE zone_id = ? ORDER BY type, name",
    )
    .bind(zone_id)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn dns_upsert(conn: &mut SqliteConnection, r: &DnsRow) -> Result<(), CoreError> {
    // Types outside the cache CHECK (SRV, CAA, ...) are not cached; the API stays the source of truth.
    if !["A", "AAAA", "CNAME", "TXT", "MX", "NS"].contains(&r.record_type.as_str()) {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO cloudflare_dns_records (id, zone_id, type, name, content, ttl, proxied, priority,
            comment, synced_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET type = excluded.type, name = excluded.name,
            content = excluded.content, ttl = excluded.ttl, proxied = excluded.proxied,
            priority = excluded.priority, comment = excluded.comment, synced_at = excluded.synced_at",
    )
    .bind(&r.id)
    .bind(&r.zone_id)
    .bind(&r.record_type)
    .bind(&r.name)
    .bind(&r.content)
    .bind(r.ttl)
    .bind(r.proxied)
    .bind(r.priority)
    .bind(&r.comment)
    .bind(r.synced_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn dns_delete(conn: &mut SqliteConnection, id: &str) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM cloudflare_dns_records WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn dns_clear_zone(conn: &mut SqliteConnection, zone_id: &str) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM cloudflare_dns_records WHERE zone_id = ?")
        .bind(zone_id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

// ---- hostnames ----------------------------------------------------------------------------

/// A Hostname with the declared port it targets (`host_port` is `None` for the registry).
#[derive(Debug, Clone, FromRow)]
pub struct HostnameRow {
    pub id: String,
    pub target_kind: String,
    pub project_id: Option<String>,
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
    pub created_at: i64,
    pub updated_at: i64,
    pub host_port: Option<i64>,
    pub bind_address: Option<String>,
    pub project_slug: Option<String>,
    pub cf_tunnel_id: Option<String>,
}

const HOSTNAME: &str = "SELECT h.id, h.target_kind, h.project_id, h.project_port_id, h.hostname,
    h.ingress_provider, h.tunnel_id, h.zone_id, h.manage_dns, h.dns_record_id, h.enabled,
    h.sync_status, h.last_error, h.last_published_at, h.created_at, h.updated_at,
    p.host_port AS host_port, p.bind_address AS bind_address, pr.slug AS project_slug,
    t.cf_tunnel_id AS cf_tunnel_id
    FROM hostnames h
    LEFT JOIN project_ports p ON p.id = h.project_port_id
    LEFT JOIN projects pr ON pr.id = h.project_id
    LEFT JOIN cloudflare_tunnels t ON t.id = h.tunnel_id";

pub struct NewHostname<'a> {
    pub id: &'a str,
    pub target_kind: &'a str,
    pub project_id: Option<&'a str>,
    pub project_port_id: Option<&'a str>,
    pub hostname: &'a str,
    pub provider: &'a str,
    pub tunnel_id: Option<&'a str>,
    pub zone_id: Option<&'a str>,
    pub manage_dns: bool,
}

pub async fn hostname_insert(
    conn: &mut SqliteConnection,
    h: &NewHostname<'_>,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO hostnames (id, target_kind, project_id, project_port_id, hostname, ingress_provider,
            tunnel_id, zone_id, manage_dns, enabled, sync_status, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 'pending', ?, ?)",
    )
    .bind(h.id)
    .bind(h.target_kind)
    .bind(h.project_id)
    .bind(h.project_port_id)
    .bind(h.hostname)
    .bind(h.provider)
    .bind(h.tunnel_id)
    .bind(h.zone_id)
    .bind(h.manage_dns)
    .bind(now)
    .bind(now)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("this hostname already exists")
        } else {
            db_err(e)
        }
    })
}

pub async fn hostname_get(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<HostnameRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{HOSTNAME} WHERE h.id = ?")))
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn hostname_list(conn: &mut SqliteConnection) -> Result<Vec<HostnameRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{HOSTNAME} ORDER BY h.hostname"
    )))
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn hostname_list_for_project(
    conn: &mut SqliteConnection,
    project_id: &str,
) -> Result<Vec<HostnameRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{HOSTNAME} WHERE h.project_id = ? ORDER BY h.hostname"
    )))
    .bind(project_id)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn hostnames_for_project_count(
    conn: &mut SqliteConnection,
    project_id: &str,
) -> Result<i64, CoreError> {
    sqlx::query_scalar("SELECT COUNT(*) FROM hostnames WHERE project_id = ?")
        .bind(project_id)
        .fetch_one(conn)
        .await
        .map_err(db_err)
}

pub async fn hostname_set_enabled(
    conn: &mut SqliteConnection,
    id: &str,
    enabled: bool,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE hostnames SET enabled = ?, updated_at = ? WHERE id = ?")
        .bind(enabled)
        .bind(now)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

/// Record the result of a publish.
pub async fn hostname_set_sync(
    conn: &mut SqliteConnection,
    id: &str,
    status: &str,
    error: Option<&str>,
    published: bool,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE hostnames SET sync_status = ?, last_error = ?, updated_at = ?,
            last_published_at = CASE WHEN ? THEN ? ELSE last_published_at END WHERE id = ?",
    )
    .bind(status)
    .bind(error)
    .bind(now)
    .bind(published)
    .bind(now)
    .bind(id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn hostname_set_dns_record(
    conn: &mut SqliteConnection,
    id: &str,
    record_id: Option<&str>,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE hostnames SET dns_record_id = ? WHERE id = ?")
        .bind(record_id)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn hostname_delete(conn: &mut SqliteConnection, id: &str) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM hostnames WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

// ---- FRP settings -------------------------------------------------------------------------

pub async fn frps_public_host(conn: &mut SqliteConnection) -> Result<String, CoreError> {
    sqlx::query_scalar("SELECT public_host FROM frps_settings WHERE id = 1")
        .fetch_one(conn)
        .await
        .map_err(db_err)
}

pub async fn frps_set_public_host(
    conn: &mut SqliteConnection,
    host: &str,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE frps_settings SET public_host = ?, updated_at = ? WHERE id = 1")
        .bind(host)
        .bind(now)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

// ---- project ports ------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct PortTarget {
    pub id: String,
    pub host_port: i64,
    pub bind_address: String,
    pub protocol: String,
}

/// The declared port `port_id` of Project `project_id`, if it exists.
pub async fn port_of_project(
    conn: &mut SqliteConnection,
    project_id: &str,
    port_id: &str,
) -> Result<Option<PortTarget>, CoreError> {
    sqlx::query_as(
        "SELECT id, host_port, bind_address, protocol FROM project_ports WHERE id = ? AND project_id = ?",
    )
    .bind(port_id)
    .bind(project_id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}
