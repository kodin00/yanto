//! API Tokens (docs/rewrite/03 a.5, D23, D29): hashed at rest, scope `read < write < admin`,
//! belonging to a user and capped by that user's permissions. Created, listed and revoked
//! only through the dashboard/CLI (never through a token); Members manage their own, the
//! Owner can list and revoke every token. Bearer authentication also lives here.

use serde::Serialize;
use serde_json::json;

use super::audit::{self, AuditActor, AuditEvent};
use super::{Ctx, DAY_MS, actor_for_user, ip_key};
use crate::actor::{Access, Actor, ActorKind, OpSpec, Origin, Scope};
use crate::crypto::{random_hex, sha256_hex};
use crate::db::repo::api_tokens::{self, ApiTokenRow, NewToken};
use crate::db::repo::db_err;
use crate::db::repo::users::{self, UserStatus};
use crate::error::CoreError;
use crate::secrets::Secret;

pub const OP_API_TOKEN_CREATE: OpSpec =
    OpSpec::new("api_token_create", Access::Authenticated, Scope::Read).session_only();
pub const OP_API_TOKEN_LIST: OpSpec =
    OpSpec::new("api_token_list", Access::Authenticated, Scope::Read).session_only();
pub const OP_API_TOKEN_REVOKE: OpSpec =
    OpSpec::new("api_token_revoke", Access::Authenticated, Scope::Read).session_only();
pub const OP_WHOAMI: OpSpec = OpSpec::new("whoami", Access::Authenticated, Scope::Read);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_API_TOKEN_CREATE,
    &OP_API_TOKEN_LIST,
    &OP_API_TOKEN_REVOKE,
    &OP_WHOAMI,
];

const TOKEN_PREFIX: &str = "yat_";
const ID_HEX_LEN: usize = 24;
const SECRET_HEX_LEN: usize = 64;
const TOUCH_INTERVAL_MS: i64 = 60_000;

#[derive(Debug, Clone, Serialize)]
pub struct TokenDto {
    pub id: String,
    pub name: String,
    /// First 8 characters of the token id part, for recognition only.
    pub hint: String,
    pub scope: String,
    pub user_id: String,
    pub username: String,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub created_at: i64,
}

impl From<ApiTokenRow> for TokenDto {
    fn from(r: ApiTokenRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            hint: r.token_hint,
            scope: r.scope,
            user_id: r.user_id,
            username: r.username,
            expires_at: r.expires_at,
            last_used_at: r.last_used_at,
            revoked_at: r.revoked_at,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TokenCreated {
    /// The full bearer token: shown once, never stored.
    pub token: Secret,
    pub info: TokenDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct WhoAmI {
    pub user_id: Option<String>,
    pub username: String,
    pub role: &'static str,
    pub token: Option<WhoAmIToken>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WhoAmIToken {
    pub id: String,
    pub name: String,
    pub scope: &'static str,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TokenService {
    ctx: Ctx,
}

impl TokenService {
    pub(super) fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    /// Create a token for the calling user (dashboard only). `expires_in_days` of `None` = no expiry.
    pub async fn create(
        &self,
        actor: &Actor,
        name: &str,
        scope: Scope,
        expires_in_days: Option<u32>,
    ) -> Result<TokenCreated, CoreError> {
        actor.authorize(&OP_API_TOKEN_CREATE, None)?;
        let user_id = actor.user_id.as_deref().ok_or_else(|| {
            CoreError::validation("actor", "API Tokens belong to a user; sign in as one")
        })?;
        let name = validate_name(name)?;
        if expires_in_days == Some(0) || expires_in_days > Some(3650) {
            return Err(CoreError::validation(
                "expires_in_days",
                "expiry must be 1-3650 days",
            ));
        }
        let now = self.ctx.now();
        let id_part = random_hex::<12>();
        let secret = random_hex::<32>();
        let token = format!("{TOKEN_PREFIX}{id_part}_{secret}");
        let id = format!("tok_{id_part}");
        let expires_at = expires_in_days.map(|d| now + i64::from(d) * DAY_MS);

        let mut tx = self.ctx.db.begin_write().await?;
        api_tokens::insert(
            &mut tx,
            &NewToken {
                id: &id,
                name: &name,
                token_hash: &sha256_hex(&token),
                token_hint: &id_part[..8],
                scope: scope.as_str(),
                user_id,
                expires_at,
                now,
            },
        )
        .await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("api_token.create", "api_token")
                .entity(&id)
                .meta(json!({ "name": name, "scope": scope.as_str(), "expires_at": expires_at })),
        )
        .await?;
        let row = api_tokens::get(&mut tx, &id).await?.ok_or_else(|| {
            CoreError::new(crate::ErrorKind::Internal, "token vanished after insert")
        })?;
        tx.commit().await.map_err(db_err)?;
        Ok(TokenCreated {
            token: Secret::new(token),
            info: row.into(),
        })
    }

    /// Owner/CLI: every token. Member: their own.
    pub async fn list(&self, actor: &Actor) -> Result<Vec<TokenDto>, CoreError> {
        actor.authorize(&OP_API_TOKEN_LIST, None)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let rows = if actor.is_owner() {
            api_tokens::list_all(&mut conn).await?
        } else {
            let user_id = actor
                .user_id
                .as_deref()
                .ok_or_else(|| CoreError::validation("actor", "API Tokens belong to a user"))?;
            api_tokens::list_for_user(&mut conn, user_id).await?
        };
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Owner: any token. Member: own tokens only (anyone else's is 404).
    pub async fn revoke(&self, actor: &Actor, token_id: &str) -> Result<(), CoreError> {
        actor.authorize(&OP_API_TOKEN_REVOKE, None)?;
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        let row = api_tokens::get(&mut tx, token_id)
            .await?
            .filter(|r| actor.is_owner() || actor.user_id.as_deref() == Some(r.user_id.as_str()))
            .ok_or_else(|| CoreError::not_found("token not found"))?;
        if !api_tokens::revoke(&mut tx, token_id, now).await? {
            return Err(CoreError::not_found("token not found"));
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("api_token.revoke", "api_token")
                .entity(token_id)
                .meta(json!({ "name": row.name, "owner": row.username })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    pub async fn whoami(&self, actor: &Actor) -> Result<WhoAmI, CoreError> {
        actor.authorize(&OP_WHOAMI, None)?;
        let token = match &actor.kind {
            ActorKind::ApiToken { token_id, .. } => {
                let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
                api_tokens::get(&mut conn, token_id)
                    .await?
                    .map(|r| WhoAmIToken {
                        scope: actor.token_scope().map_or("read", Scope::as_str),
                        id: r.id,
                        name: r.name,
                        expires_at: r.expires_at,
                    })
            }
            _ => None,
        };
        Ok(WhoAmI {
            user_id: actor.user_id.clone(),
            username: actor.label.clone(),
            role: actor.role.as_str(),
            token,
        })
    }

    /// Resolve `Authorization: Bearer <token>` to an [`Actor`] acting as the token's user.
    /// Failures are rate limited per client IP (20 / 15 min).
    pub async fn authenticate(&self, origin: &Origin, raw: &str) -> Result<Actor, CoreError> {
        let ctx = &self.ctx;
        let ik = ip_key(origin);
        ctx.limits
            .bearer_ip
            .check(&ik, ctx.now())
            .map_err(CoreError::rate_limited)?;
        match self.authenticate_inner(origin, raw).await {
            Ok(a) => Ok(a),
            Err(e) if matches!(e.kind, crate::ErrorKind::Unauthenticated) => {
                if ctx.limits.bearer_ip.record_failure(&ik, ctx.now()) {
                    let ev = AuditEvent::new("api_token.auth_failed", "api_token")
                        .denied()
                        .meta(json!({ "reason": "rate_limited" }));
                    audit::write_standalone(ctx, &AuditActor::unknown(origin, "<unknown>"), &ev)
                        .await?;
                }
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    async fn authenticate_inner(&self, origin: &Origin, raw: &str) -> Result<Actor, CoreError> {
        let bad = || CoreError::unauthenticated("invalid or expired API token");
        let rest = raw.strip_prefix(TOKEN_PREFIX).ok_or_else(bad)?;
        let (id_part, secret) = rest.split_once('_').ok_or_else(bad)?;
        let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
        if !hex(id_part, ID_HEX_LEN) || !hex(secret, SECRET_HEX_LEN) {
            return Err(bad());
        }
        let now = self.ctx.now();
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let row = api_tokens::find_active_by_hash(&mut conn, &sha256_hex(raw), now)
            .await?
            .ok_or_else(bad)?;
        let user = users::get(&mut conn, &row.user_id).await?.ok_or_else(bad)?;
        if user.status() != UserStatus::Active {
            return Err(bad());
        }
        let scope = row.scope.parse::<Scope>().map_err(|_| bad())?;
        let actor = actor_for_user(
            &mut conn,
            &user,
            ActorKind::ApiToken {
                token_id: row.id.clone(),
                token_name: row.name.clone(),
                scope,
            },
            origin,
        )
        .await?;
        drop(conn);
        if row
            .last_used_at
            .is_none_or(|t| now - t >= TOUCH_INTERVAL_MS)
        {
            let res = async {
                let mut tx = self.ctx.db.begin_write().await?;
                api_tokens::touch_last_used(&mut tx, &row.id, now, origin.ip.as_deref()).await?;
                tx.commit().await.map_err(db_err)
            }
            .await;
            if let Err(err) = res {
                tracing::warn!(error = %err, "could not update token last_used_at");
            }
        }
        Ok(actor)
    }
}

fn validate_name(raw: &str) -> Result<String, CoreError> {
    let n = raw.trim();
    if n.is_empty() || n.chars().count() > 100 || n.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "name",
            "name must be 1-100 characters",
        ));
    }
    Ok(n.to_owned())
}
