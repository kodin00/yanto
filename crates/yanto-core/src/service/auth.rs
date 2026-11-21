//! Sign-in, sessions, setup code / first Owner, invite and reset acceptance,
//! password change and `owner-reset` (docs/rewrite/03 a.3, 05 section 3.6).

use serde::Serialize;
use serde_json::json;

use super::audit::{self, AuditActor, AuditEvent};
use super::{Ctx, DAY_MS, actor_for_user, ip_key};
use crate::actor::{Access, Actor, ActorKind, OpSpec, Origin, Role, Scope};
use crate::crypto::{ct_eq, new_id, random_bytes, random_token_b64, sha256_hex};
use crate::db::repo::sessions::{self, NewSession};
use crate::db::repo::users::{self, UserRow, UserStatus};
use crate::db::repo::{account_tokens, db_err, settings};
use crate::error::CoreError;
use crate::password::{MAX_PASSWORD_BYTES, Passwords};
use crate::secrets::Secret;

pub const OP_OWNER_SETUP: OpSpec = OpSpec::new("owner_setup", Access::Public, Scope::Read);
pub const OP_SESSION_LOGIN: OpSpec = OpSpec::new("session_login", Access::Public, Scope::Read);
pub const OP_ACCOUNT_PREVIEW: OpSpec = OpSpec::new("account_preview", Access::Public, Scope::Read);
pub const OP_ACCOUNT_SETUP: OpSpec = OpSpec::new("account_setup", Access::Public, Scope::Read);
pub const OP_SESSION_LOGOUT: OpSpec =
    OpSpec::new("session_logout", Access::Authenticated, Scope::Read).session_only();
pub const OP_SESSION_LIST: OpSpec =
    OpSpec::new("session_list", Access::Authenticated, Scope::Read).session_only();
pub const OP_SESSION_REVOKE: OpSpec =
    OpSpec::new("session_revoke", Access::Authenticated, Scope::Read).session_only();
pub const OP_PASSWORD_CHANGE: OpSpec =
    OpSpec::new("password_change", Access::Authenticated, Scope::Read).session_only();
pub const OP_SETUP_CODE_ISSUE: OpSpec =
    OpSpec::new("setup_code_issue", Access::Owner, Scope::Admin).session_only();
pub const OP_OWNER_RESET: OpSpec =
    OpSpec::new("owner_reset", Access::Owner, Scope::Admin).session_only();

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_OWNER_SETUP,
    &OP_SESSION_LOGIN,
    &OP_ACCOUNT_PREVIEW,
    &OP_ACCOUNT_SETUP,
    &OP_SESSION_LOGOUT,
    &OP_SESSION_LIST,
    &OP_SESSION_REVOKE,
    &OP_PASSWORD_CHANGE,
    &OP_SETUP_CODE_ISSUE,
    &OP_OWNER_RESET,
];

pub const SETUP_CODE_HASH_KEY: &str = "setup.code_hash";
pub const SETUP_CODE_EXPIRES_KEY: &str = "setup.code_expires_at";
const SETUP_CODE_TTL_MS: i64 = DAY_MS;
pub const ACCOUNT_TOKEN_TTL_MS: i64 = DAY_MS;
/// 32 symbols (no `I`, `O`, `0`, `1`): 5 bits per char, 16 chars = 80 bits, no modulo bias.
const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const TOUCH_INTERVAL_MS: i64 = 60_000;
const BAD_LOGIN: &str = "Invalid username or password";
const BAD_LINK: &str = "This link is invalid or has expired";
const BAD_CODE: &str = "Invalid or expired setup code";

/// A fresh session cookie value plus its CSRF token.
#[derive(Debug, Clone)]
pub struct SessionGrant {
    /// Value for the `yanto_session` cookie. Only its SHA-256 is stored.
    pub cookie: Secret,
    pub csrf_token: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct LoginOutcome {
    pub user_id: String,
    pub username: String,
    pub role: Role,
    pub session: SessionGrant,
}

/// An authenticated dashboard request.
#[derive(Debug, Clone)]
pub struct SessionAuth {
    pub actor: Actor,
    pub session_id: String,
    pub csrf_token: String,
}

/// A one-time link shown exactly once to whoever issued it.
#[derive(Debug, Clone)]
pub struct AccountLink {
    pub url: Secret,
    pub expires_at: i64,
    pub username: String,
}

#[derive(Debug, Clone)]
pub struct SetupCodeIssued {
    /// Formatted `XXXX-XXXX-XXXX-XXXX`. A live credential until used.
    pub code: Secret,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountPreview {
    pub username: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionDto {
    pub id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_seen_at: i64,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub current: bool,
}

#[derive(Debug, Clone)]
pub struct AuthService {
    ctx: Ctx,
}

impl AuthService {
    pub(super) fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    // ---- setup code and first Owner -------------------------------------

    /// `true` while no Owner exists (the dashboard then offers `/setup`).
    pub async fn setup_pending(&self) -> Result<bool, CoreError> {
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        Ok(users::owner(&mut conn).await?.is_none())
    }

    /// Issue a new one-time setup code, replacing any earlier one. Only while no Owner exists.
    /// The plaintext is returned once and never stored or logged.
    pub async fn issue_setup_code(&self, actor: &Actor) -> Result<SetupCodeIssued, CoreError> {
        actor.authorize(&OP_SETUP_CODE_ISSUE, None)?;
        let now = self.ctx.now();
        let code = generate_setup_code();
        let expires_at = now + SETUP_CODE_TTL_MS;
        let mut tx = self.ctx.db.begin_write().await?;
        if users::owner(&mut tx).await?.is_some() {
            return Err(CoreError::conflict(
                "an Owner already exists; use `yanto owner-reset` to recover access",
            ));
        }
        settings::set(
            &mut tx,
            SETUP_CODE_HASH_KEY,
            &json!(sha256_hex(&normalize_code(&code))).to_string(),
            now,
        )
        .await?;
        settings::set(
            &mut tx,
            SETUP_CODE_EXPIRES_KEY,
            &expires_at.to_string(),
            now,
        )
        .await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("auth.setup_code_issue", "setup"),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(SetupCodeIssued {
            code: Secret::new(code),
            expires_at,
        })
    }

    /// Create the one Owner. Needs a valid setup code, consumed atomically with the insert.
    pub async fn owner_setup(
        &self,
        origin: &Origin,
        code: &str,
        username: &str,
        password: &str,
        user_agent: Option<&str>,
    ) -> Result<LoginOutcome, CoreError> {
        let ctx = &self.ctx;
        let ik = ip_key(origin);
        check_limit(ctx.limits.setup_ip.check(&ik, ctx.now()))?;
        let username = super::users::validate_username(username)?;
        Passwords::validate_policy(password)?;

        if !self.setup_code_valid(code).await? {
            self.fail_setup(origin, &ik, "owner_setup").await?;
            return Err(CoreError::unauthenticated(BAD_CODE));
        }
        let hash = ctx.passwords.hash(password).await?;

        let now = ctx.now();
        let mut tx = ctx.db.begin_write().await?;
        if users::owner(&mut tx).await?.is_some() {
            return Err(CoreError::conflict("setup has already been completed"));
        }
        // Re-check inside the transaction so two concurrent setups cannot both win.
        if !code_matches(&mut tx, code, now).await? {
            drop(tx);
            self.fail_setup(origin, &ik, "owner_setup").await?;
            return Err(CoreError::unauthenticated(BAD_CODE));
        }
        let id = new_id("usr");
        users::insert(
            &mut tx,
            &id,
            &username,
            Role::Owner,
            UserStatus::Active,
            Some(&hash),
            now,
        )
        .await?;
        settings::delete(&mut tx, SETUP_CODE_HASH_KEY).await?;
        settings::delete(&mut tx, SETUP_CODE_EXPIRES_KEY).await?;
        let session = create_session(ctx, &mut tx, &id, origin, user_agent).await?;
        users::touch_login(&mut tx, &id, now).await?;
        audit::write(
            ctx,
            &mut tx,
            &AuditActor::user(&id, &username, origin),
            &AuditEvent::new("auth.setup_owner", "user").entity(&id),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        ctx.limits.setup_ip.clear(&ik);
        Ok(LoginOutcome {
            user_id: id,
            username,
            role: Role::Owner,
            session,
        })
    }

    async fn setup_code_valid(&self, code: &str) -> Result<bool, CoreError> {
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        code_matches(&mut conn, code, self.ctx.now()).await
    }

    async fn fail_setup(&self, origin: &Origin, ik: &str, what: &str) -> Result<(), CoreError> {
        let now = self.ctx.now();
        let tripped = self.ctx.limits.setup_ip.record_failure(ik, now);
        let actor = AuditActor::unknown(origin, "<unknown>");
        audit::write_standalone(
            &self.ctx,
            &actor,
            &AuditEvent::new("auth.setup_failed", "setup")
                .denied()
                .meta(json!({ "operation": what })),
        )
        .await?;
        if tripped {
            audit::write_standalone(
                &self.ctx,
                &actor,
                &AuditEvent::new("auth.rate_limited", "setup")
                    .denied()
                    .meta(json!({ "scope": "setup" })),
            )
            .await?;
        }
        Ok(())
    }

    // ---- login / logout ---------------------------------------------------

    pub async fn login(
        &self,
        origin: &Origin,
        username: &str,
        password: &str,
        user_agent: Option<&str>,
    ) -> Result<LoginOutcome, CoreError> {
        let ctx = &self.ctx;
        let now = ctx.now();
        let ik = ip_key(origin);
        let uk = username.trim().to_lowercase();
        check_limit(ctx.limits.login_ip.check(&ik, now))?;
        check_limit(ctx.limits.login_user.check(&uk, now))?;

        let user = if uk.is_empty() || uk.len() > 200 {
            None
        } else {
            let mut conn = ctx.db.reader().acquire().await.map_err(db_err)?;
            users::by_username(&mut conn, username.trim()).await?
        };
        let valid = match &user {
            Some(u) if u.status() == UserStatus::Active && password.len() <= MAX_PASSWORD_BYTES => {
                match &u.password_hash {
                    Some(h) => ctx.passwords.verify(password, h).await,
                    None => false,
                }
            }
            _ => {
                ctx.passwords
                    .verify_dummy(&password[..password.len().min(MAX_PASSWORD_BYTES)])
                    .await;
                false
            }
        };

        let now = ctx.now();
        let Some(user) = user.filter(|_| valid) else {
            let trip_ip = ctx.limits.login_ip.record_failure(&ik, now);
            let trip_user = ctx.limits.login_user.record_failure(&uk, now);
            let found = self.find_for_audit(username).await?;
            let actor = match &found {
                Some(u) => AuditActor::user(&u.id, &u.username, origin),
                None => AuditActor::unknown(origin, "<unknown>"),
            };
            let mut tx = ctx.db.begin_write().await?;
            audit::write(
                ctx,
                &mut tx,
                &actor,
                &AuditEvent::new("auth.login_failed", "session").denied(),
            )
            .await?;
            if trip_ip || trip_user {
                audit::write(
                    ctx,
                    &mut tx,
                    &actor,
                    &AuditEvent::new("auth.rate_limited", "session")
                        .denied()
                        .meta(json!({ "scope": if trip_ip { "ip" } else { "username" } })),
                )
                .await?;
            }
            tx.commit().await.map_err(db_err)?;
            return Err(CoreError::unauthenticated(BAD_LOGIN));
        };

        ctx.limits.login_user.clear(&uk);
        let mut tx = ctx.db.begin_write().await?;
        let session = create_session(ctx, &mut tx, &user.id, origin, user_agent).await?;
        users::touch_login(&mut tx, &user.id, now).await?;
        audit::write(
            ctx,
            &mut tx,
            &AuditActor::user(&user.id, &user.username, origin),
            &AuditEvent::new("auth.login", "session").entity(&user.id),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(LoginOutcome {
            role: user.role(),
            user_id: user.id,
            username: user.username,
            session,
        })
    }

    async fn find_for_audit(&self, username: &str) -> Result<Option<UserRow>, CoreError> {
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        users::by_username(&mut conn, username.trim()).await
    }

    pub async fn logout(&self, actor: &Actor, session_id: &str) -> Result<(), CoreError> {
        actor.authorize(&OP_SESSION_LOGOUT, None)?;
        let mut tx = self.ctx.db.begin_write().await?;
        sessions::delete(&mut tx, session_id).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("auth.logout", "session"),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    /// Resolve a `yanto_session` cookie value. Any problem is `Unauthenticated`.
    pub async fn authenticate_session(
        &self,
        origin: &Origin,
        cookie: &str,
    ) -> Result<SessionAuth, CoreError> {
        let ctx = &self.ctx;
        let denied = || CoreError::unauthenticated("not signed in");
        if cookie.is_empty() || cookie.len() > 256 {
            return Err(denied());
        }
        let id = sha256_hex(cookie);
        let now = ctx.now();
        let policy = ctx.session_policy;
        let mut conn = ctx.db.reader().acquire().await.map_err(db_err)?;
        let session = sessions::get(&mut conn, &id).await?.ok_or_else(denied)?;
        if now >= session.expires_at || now - session.last_seen_at > policy.idle_ms {
            drop(conn);
            self.delete_session_quietly(&id).await;
            return Err(denied());
        }
        let user = users::get(&mut conn, &session.user_id)
            .await?
            .ok_or_else(denied)?;
        if user.status() != UserStatus::Active {
            return Err(denied());
        }
        let actor = actor_for_user(&mut conn, &user, ActorKind::User, origin).await?;
        drop(conn);
        if now - session.last_seen_at >= TOUCH_INTERVAL_MS
            && let Err(err) = self.touch_session(&id, now).await
        {
            tracing::warn!(error = %err, "could not update session last_seen_at");
        }
        Ok(SessionAuth {
            actor,
            session_id: id,
            csrf_token: session.csrf_token,
        })
    }

    async fn touch_session(&self, id: &str, now: i64) -> Result<(), CoreError> {
        let mut tx = self.ctx.db.begin_write().await?;
        sessions::touch(&mut tx, id, now).await?;
        tx.commit().await.map_err(db_err)
    }

    async fn delete_session_quietly(&self, id: &str) {
        let res = async {
            let mut tx = self.ctx.db.begin_write().await?;
            sessions::delete(&mut tx, id).await?;
            tx.commit().await.map_err(db_err)
        }
        .await;
        if let Err(err) = res {
            tracing::warn!(error = %err, "could not delete expired session");
        }
    }

    // ---- own sessions and password ----------------------------------------

    pub async fn session_list(
        &self,
        actor: &Actor,
        current_session_id: &str,
    ) -> Result<Vec<SessionDto>, CoreError> {
        actor.authorize(&OP_SESSION_LIST, None)?;
        let user_id = actor.user_id.as_deref().ok_or_else(no_user)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let rows = sessions::list_for_user(&mut conn, user_id).await?;
        Ok(rows
            .into_iter()
            .map(|r| SessionDto {
                current: r.id == current_session_id,
                id: r.id,
                created_at: r.created_at,
                expires_at: r.expires_at,
                last_seen_at: r.last_seen_at,
                ip: r.ip,
                user_agent: r.user_agent,
            })
            .collect())
    }

    /// Revoke one of the caller's own sessions (404 for anyone else's).
    pub async fn session_revoke(&self, actor: &Actor, session_id: &str) -> Result<(), CoreError> {
        actor.authorize(&OP_SESSION_REVOKE, None)?;
        let user_id = actor.user_id.as_deref().ok_or_else(no_user)?;
        let mut tx = self.ctx.db.begin_write().await?;
        if !sessions::delete_owned(&mut tx, session_id, user_id).await? {
            return Err(CoreError::not_found("session not found"));
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("auth.session_revoke", "session"),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    /// Change the caller's password; every other session of the user is revoked.
    pub async fn password_change(
        &self,
        actor: &Actor,
        current_session_id: &str,
        current: &str,
        new: &str,
    ) -> Result<(), CoreError> {
        actor.authorize(&OP_PASSWORD_CHANGE, None)?;
        let user_id = actor.user_id.as_deref().ok_or_else(no_user)?;
        Passwords::validate_policy(new)?;
        let user = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            users::get(&mut conn, user_id).await?
        }
        .ok_or_else(|| CoreError::not_found("user not found"))?;
        let ok = match &user.password_hash {
            Some(h) if current.len() <= MAX_PASSWORD_BYTES => {
                self.ctx.passwords.verify(current, h).await
            }
            _ => false,
        };
        if !ok {
            return Err(CoreError::validation(
                "current_password",
                "current password is incorrect",
            ));
        }
        let hash = self.ctx.passwords.hash(new).await?;
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        users::set_password_active(&mut tx, user_id, &hash, now).await?;
        let revoked =
            sessions::delete_for_user_except(&mut tx, user_id, current_session_id).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("auth.password_change", "user")
                .entity(user_id)
                .meta(json!({ "sessions_revoked": revoked })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    // ---- invite / reset acceptance ------------------------------------------

    /// Look at a link without consuming it (POST, so the token stays out of URLs and logs).
    pub async fn account_preview(
        &self,
        origin: &Origin,
        token: &str,
    ) -> Result<AccountPreview, CoreError> {
        let ik = ip_key(origin);
        check_limit(self.ctx.limits.setup_ip.check(&ik, self.ctx.now()))?;
        match self.lookup_token(token).await? {
            Some((tok, user)) => Ok(AccountPreview {
                username: user.username,
                purpose: tok.purpose,
            }),
            None => {
                self.fail_setup(origin, &ik, "account_preview").await?;
                Err(CoreError::unauthenticated(BAD_LINK))
            }
        }
    }

    async fn lookup_token(
        &self,
        token: &str,
    ) -> Result<Option<(account_tokens::AccountTokenRow, UserRow)>, CoreError> {
        if token.is_empty() || token.len() > 256 {
            return Ok(None);
        }
        let now = self.ctx.now();
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let Some(tok) = account_tokens::by_hash(&mut conn, &sha256_hex(token)).await? else {
            return Ok(None);
        };
        if tok.used_at.is_some() || tok.expires_at <= now {
            return Ok(None);
        }
        let Some(user) = users::get(&mut conn, &tok.user_id).await? else {
            return Ok(None);
        };
        if user.status() == UserStatus::Disabled {
            return Ok(None);
        }
        Ok(Some((tok, user)))
    }

    /// Accept an invite or reset: set the password, mark the link used, sign in.
    pub async fn account_setup(
        &self,
        origin: &Origin,
        token: &str,
        password: &str,
        user_agent: Option<&str>,
    ) -> Result<LoginOutcome, CoreError> {
        let ctx = &self.ctx;
        let ik = ip_key(origin);
        check_limit(ctx.limits.setup_ip.check(&ik, ctx.now()))?;
        Passwords::validate_policy(password)?;
        let Some((tok, user)) = self.lookup_token(token).await? else {
            self.fail_setup(origin, &ik, "account_setup").await?;
            return Err(CoreError::unauthenticated(BAD_LINK));
        };
        let hash = ctx.passwords.hash(password).await?;
        let now = ctx.now();
        let mut tx = ctx.db.begin_write().await?;
        if !account_tokens::claim(&mut tx, &tok.id, now).await? {
            drop(tx);
            self.fail_setup(origin, &ik, "account_setup").await?;
            return Err(CoreError::unauthenticated(BAD_LINK));
        }
        users::set_password_active(&mut tx, &user.id, &hash, now).await?;
        account_tokens::mark_all_used(&mut tx, &user.id, now).await?;
        sessions::delete_for_user(&mut tx, &user.id).await?;
        let session = create_session(ctx, &mut tx, &user.id, origin, user_agent).await?;
        users::touch_login(&mut tx, &user.id, now).await?;
        audit::write(
            ctx,
            &mut tx,
            &AuditActor::user(&user.id, &user.username, origin),
            &AuditEvent::new("auth.account_setup", "user")
                .entity(&user.id)
                .meta(json!({ "purpose": tok.purpose })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(LoginOutcome {
            role: user.role(),
            user_id: user.id,
            username: user.username,
            session,
        })
    }

    // ---- owner reset (CLI) ----------------------------------------------------

    /// Revoke the Owner's sessions and return a one-time reset link (24h). CLI only.
    pub async fn owner_reset(&self, actor: &Actor) -> Result<AccountLink, CoreError> {
        actor.authorize(&OP_OWNER_RESET, None)?;
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        let owner = users::owner(&mut tx).await?.ok_or_else(|| {
            CoreError::not_found("no Owner exists yet; run `yanto setup-code` instead")
        })?;
        users::clear_password_invited(&mut tx, &owner.id, now).await?;
        let revoked = sessions::delete_for_user(&mut tx, &owner.id).await?;
        account_tokens::mark_all_used(&mut tx, &owner.id, now).await?;
        let (url, expires_at) = issue_account_token(&self.ctx, &mut tx, &owner.id, "reset").await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("auth.owner_reset", "user")
                .entity(&owner.id)
                .meta(json!({ "sessions_revoked": revoked })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(AccountLink {
            url,
            expires_at,
            username: owner.username,
        })
    }

    // ---- maintenance ---------------------------------------------------------------

    /// Delete expired sessions and account tokens. Called hourly by `serve`.
    pub async fn purge_expired(&self) -> Result<u64, CoreError> {
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        let a =
            sessions::delete_expired(&mut tx, now, now - self.ctx.session_policy.idle_ms).await?;
        let b = account_tokens::delete_expired(&mut tx, now).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(a + b)
    }
}

fn no_user() -> CoreError {
    CoreError::validation("actor", "this operation needs a signed-in user")
}

fn check_limit(r: Result<(), u64>) -> Result<(), CoreError> {
    r.map_err(CoreError::rate_limited)
}

/// Create a session row and return the cookie value (shown once) and CSRF token.
pub(crate) async fn create_session(
    ctx: &Ctx,
    conn: &mut sqlx::SqliteConnection,
    user_id: &str,
    origin: &Origin,
    user_agent: Option<&str>,
) -> Result<SessionGrant, CoreError> {
    let now = ctx.now();
    let cookie = random_token_b64();
    let csrf = random_token_b64();
    let expires_at = now + ctx.session_policy.absolute_ms;
    let ua: Option<String> = user_agent.map(|u| u.chars().take(256).collect());
    sessions::insert(
        conn,
        &NewSession {
            id: &sha256_hex(&cookie),
            user_id,
            csrf_token: &csrf,
            now,
            expires_at,
            ip: origin.ip.as_deref(),
            user_agent: ua.as_deref(),
        },
    )
    .await?;
    Ok(SessionGrant {
        cookie: Secret::new(cookie),
        csrf_token: csrf,
        expires_at,
    })
}

/// Insert a single-use account token and return the full link plus expiry.
pub(crate) async fn issue_account_token(
    ctx: &Ctx,
    conn: &mut sqlx::SqliteConnection,
    user_id: &str,
    purpose: &str,
) -> Result<(Secret, i64), CoreError> {
    let now = ctx.now();
    let token = random_token_b64();
    let expires_at = now + ACCOUNT_TOKEN_TTL_MS;
    account_tokens::insert(
        conn,
        &new_id("atk"),
        user_id,
        purpose,
        &sha256_hex(&token),
        expires_at,
        now,
    )
    .await?;
    Ok((
        Secret::new(format!("{}/account/setup#token={token}", ctx.base_url)),
        expires_at,
    ))
}

fn generate_setup_code() -> String {
    let bytes = random_bytes::<16>();
    let chars: Vec<char> = bytes
        .iter()
        .map(|b| CODE_ALPHABET[(b & 31) as usize] as char)
        .collect();
    chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

/// Uppercase and drop separators so `abcd-efgh ...` and `ABCDEFGH...` are equal.
fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Constant-time comparison of `code` against the stored hash, honoring expiry.
async fn code_matches(
    conn: &mut sqlx::SqliteConnection,
    code: &str,
    now: i64,
) -> Result<bool, CoreError> {
    let stored = settings::get(conn, SETUP_CODE_HASH_KEY).await?;
    let expires = settings::get(conn, SETUP_CODE_EXPIRES_KEY).await?;
    let (Some(stored), Some(expires)) = (stored, expires) else {
        return Ok(false);
    };
    let stored: String = serde_json::from_str(&stored).unwrap_or_default();
    let expires: i64 = expires.parse().unwrap_or(0);
    let given = sha256_hex(&normalize_code(code));
    Ok(ct_eq(&stored, &given) && now < expires)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_code_shape() {
        let c = generate_setup_code();
        assert_eq!(c.len(), 19);
        assert_eq!(c.matches('-').count(), 3);
        assert!(
            c.chars()
                .filter(|c| *c != '-')
                .all(|c| CODE_ALPHABET.contains(&(c as u8)))
        );
    }

    #[test]
    fn code_normalization() {
        assert_eq!(normalize_code("abcd-efgh jk23"), "ABCDEFGHJK23");
    }
}
