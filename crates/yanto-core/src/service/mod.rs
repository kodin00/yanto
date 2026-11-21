//! THE service layer (docs/rewrite/01 section 4). Every behavior is a method on a
//! service here; dashboard, REST and MCP adapters only parse input, build an
//! [`Actor`], call a service and render the result. Authorization happens inside
//! the service (`actor.authorize(&OP_X, ..)?` first), never in an adapter.
//!
//! Adding a feature module (docs/rewrite/08-conventions.md):
//! 1. create `service/<area>.rs` with its `OpSpec` constants and a `<Area>Service`;
//! 2. add `pub mod <area>;` below;
//! 3. add a field plus one line in [`Services::new`];
//! 4. add its specs to [`all_operations`].

pub mod audit;
pub mod auth;
pub mod authorized_keys;
pub mod backups;
pub mod cron;
pub mod tokens;
pub mod users;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use crate::actor::{Actor, ActorKind, OpSpec, Origin, Role};
use crate::config::Config;
use crate::db::Db;
use crate::db::repo::access;
use crate::db::repo::users::UserRow;
use crate::error::CoreError;
use crate::password::Passwords;
use crate::ports::{Clock, HostMgmtPorts};
use crate::ratelimit::FailureLimiter;
use crate::secrets::Secrets;

pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Session lifetimes (D30): 24h idle, 7 days absolute.
#[derive(Debug, Clone, Copy)]
pub struct SessionPolicy {
    pub idle_ms: i64,
    pub absolute_ms: i64,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle_ms: DAY_MS,
            absolute_ms: 7 * DAY_MS,
        }
    }
}

/// Everything the composition root hands to the services.
pub struct ServiceDeps<'a> {
    pub db: Db,
    pub config: &'a Config,
    pub clock: Arc<dyn Clock>,
    pub secrets: Arc<Secrets>,
    pub passwords: Passwords,
    /// Host side of Cron Entries, Authorized Keys and Backups (M5).
    pub host_mgmt: HostMgmtPorts,
}

/// Shared, cheaply cloneable context held by every service.
#[derive(Clone)]
pub struct Ctx {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub secrets: Arc<Secrets>,
    pub passwords: Arc<Passwords>,
    pub session_policy: SessionPolicy,
    /// Base URL used in invite/reset links: `server.public_url` or `http://bind:port`.
    pub base_url: Arc<str>,
    pub limits: Arc<Limits>,
}

impl std::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx").finish_non_exhaustive()
    }
}

/// Rate limiters (docs/rewrite/03 a.4). Per process, in memory.
#[derive(Debug)]
pub struct Limits {
    pub login_ip: FailureLimiter,
    pub login_user: FailureLimiter,
    pub setup_ip: FailureLimiter,
    pub bearer_ip: FailureLimiter,
}

impl Limits {
    fn new() -> Self {
        let w = Duration::from_secs(15 * 60);
        Self {
            login_ip: FailureLimiter::new(5, w),
            login_user: FailureLimiter::new(5, w),
            setup_ip: FailureLimiter::new(10, w),
            bearer_ip: FailureLimiter::new(20, w),
        }
    }
}

impl Ctx {
    pub fn now(&self) -> i64 {
        self.clock.now_ms()
    }
}

/// All services, built once by the composition root and shared by every adapter.
#[derive(Debug, Clone)]
pub struct Services {
    ctx: Ctx,
    pub audit: audit::AuditService,
    pub auth: auth::AuthService,
    pub users: users::UserService,
    pub tokens: tokens::TokenService,
    pub cron: cron::CronService,
    pub authorized_keys: authorized_keys::AuthorizedKeysService,
    pub backups: backups::BackupService,
}

impl Services {
    pub fn new(deps: ServiceDeps<'_>) -> Self {
        let cfg = deps.config;
        let base_url = if cfg.server.public_url.is_empty() {
            format!("http://{}:{}", cfg.server.bind, cfg.server.port)
        } else {
            cfg.server.public_url.trim_end_matches('/').to_owned()
        };
        let ctx = Ctx {
            db: deps.db,
            clock: deps.clock,
            secrets: deps.secrets,
            passwords: Arc::new(deps.passwords),
            session_policy: SessionPolicy::default(),
            base_url: base_url.into(),
            limits: Arc::new(Limits::new()),
        };
        Self {
            audit: audit::AuditService::new(ctx.clone()),
            auth: auth::AuthService::new(ctx.clone()),
            users: users::UserService::new(ctx.clone()),
            tokens: tokens::TokenService::new(ctx.clone()),
            cron: cron::CronService::new(ctx.clone(), deps.host_mgmt.clone()),
            backups: backups::BackupService::new(
                ctx.clone(),
                deps.host_mgmt.clone(),
                cfg.paths.data_dir.clone(),
            ),
            authorized_keys: authorized_keys::AuthorizedKeysService::new(
                ctx.clone(),
                deps.host_mgmt.clone(),
                cfg.ssh.authorized_keys_user.clone(),
            ),
            ctx,
        }
    }

    /// Shared context, for service modules in other crates' tests and the audit writer.
    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }
}

/// Every operation spec in the system. The permission-matrix tests iterate this
/// list and fail when an operation has no expectation row.
pub fn all_operations() -> Vec<&'static OpSpec> {
    let mut v: Vec<&'static OpSpec> = Vec::new();
    v.extend(auth::OPERATIONS);
    v.extend(users::OPERATIONS);
    v.extend(tokens::OPERATIONS);
    v.extend(cron::OPERATIONS);
    v.extend(authorized_keys::OPERATIONS);
    v.extend(backups::OPERATIONS);
    v.push(&audit::OP_AUDIT_LIST);
    v
}

/// Build the [`Actor`] for an authenticated user row, loading Member grants.
pub(crate) async fn actor_for_user(
    conn: &mut sqlx::SqliteConnection,
    user: &UserRow,
    kind: ActorKind,
    origin: &Origin,
) -> Result<Actor, CoreError> {
    let grants = if user.role() == Role::Member {
        access::load(conn, &user.id).await?
    } else {
        Default::default()
    };
    Ok(Actor {
        kind,
        user_id: Some(user.id.clone()),
        label: user.username.clone(),
        role: user.role(),
        grants,
        channel: origin.channel,
        ip: origin.ip.clone(),
        request_id: origin.request_id.clone(),
    })
}

/// Key for IP-scoped limiters when the client address is unknown.
pub(crate) fn ip_key(origin: &Origin) -> String {
    origin.ip.clone().unwrap_or_else(|| "unknown".into())
}
