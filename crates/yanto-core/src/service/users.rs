//! Owner-managed Members: invites, per-Project access, status, reset links, deletion
//! (docs/rewrite/03 a.2, c). All operations are Owner-only; over REST they need `admin` scope.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::json;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use super::auth::{AccountLink, issue_account_token};
use crate::actor::{Access, Actor, Permission, Role, Scope};
use crate::crypto::new_id;
use crate::db::repo::access::Grants;
use crate::db::repo::users::{self, UserRow, UserStatus};
use crate::db::repo::{access, account_tokens, db_err, sessions};
use crate::error::CoreError;
use crate::{actor::OpSpec, db::repo::api_tokens};

pub const OP_USER_LIST: OpSpec = OpSpec::new("user_list", Access::Owner, Scope::Admin);
pub const OP_USER_GET: OpSpec = OpSpec::new("user_get", Access::Owner, Scope::Admin);
pub const OP_USER_CREATE: OpSpec = OpSpec::new("user_create", Access::Owner, Scope::Admin);
pub const OP_USER_ACCESS_UPDATE: OpSpec =
    OpSpec::new("user_access_update", Access::Owner, Scope::Admin);
pub const OP_USER_STATUS_UPDATE: OpSpec =
    OpSpec::new("user_status_update", Access::Owner, Scope::Admin);
pub const OP_USER_RESET_LINK: OpSpec = OpSpec::new("user_reset_link", Access::Owner, Scope::Admin);
pub const OP_USER_DELETE: OpSpec = OpSpec::new("user_delete", Access::Owner, Scope::Admin);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_USER_LIST,
    &OP_USER_GET,
    &OP_USER_CREATE,
    &OP_USER_ACCESS_UPDATE,
    &OP_USER_STATUS_UPDATE,
    &OP_USER_RESET_LINK,
    &OP_USER_DELETE,
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GrantDto {
    pub project_id: String,
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserDto {
    pub id: String,
    pub username: String,
    pub role: &'static str,
    pub status: &'static str,
    pub last_login_at: Option<i64>,
    pub created_at: i64,
    pub grants: Vec<GrantDto>,
}

/// Input grant: a Project and the permissions held on it (may be empty = view only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantInput {
    pub project_id: String,
    pub permissions: Vec<Permission>,
}

#[derive(Debug, Clone)]
pub struct UserCreated {
    pub user: UserDto,
    pub invite: AccountLink,
}

#[derive(Debug, Clone)]
pub struct UserService {
    ctx: Ctx,
}

impl UserService {
    pub(super) fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    pub async fn list(&self, actor: &Actor) -> Result<Vec<UserDto>, CoreError> {
        actor.authorize(&OP_USER_LIST, None)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let rows = users::list(&mut conn).await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let grants = access::load(&mut conn, &r.id).await?;
            out.push(dto(r, &grants));
        }
        Ok(out)
    }

    pub async fn get(&self, actor: &Actor, user_id: &str) -> Result<UserDto, CoreError> {
        actor.authorize(&OP_USER_GET, None)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let row = users::get(&mut conn, user_id)
            .await?
            .ok_or_else(not_found)?;
        let grants = access::load(&mut conn, &row.id).await?;
        Ok(dto(row, &grants))
    }

    /// Create a Member in `invited` state with an invite link (shown once).
    pub async fn create(
        &self,
        actor: &Actor,
        username: &str,
        grants: &[GrantInput],
    ) -> Result<UserCreated, CoreError> {
        actor.authorize(&OP_USER_CREATE, None)?;
        let username = validate_username(username)?;
        let grants = normalize_grants(grants)?;
        let now = self.ctx.now();
        let id = new_id("usr");
        let mut tx = self.ctx.db.begin_write().await?;
        users::insert(
            &mut tx,
            &id,
            &username,
            Role::Member,
            UserStatus::Invited,
            None,
            now,
        )
        .await?;
        access::replace(&mut tx, &id, &grants, now).await?;
        let (url, expires_at) = issue_account_token(&self.ctx, &mut tx, &id, "invite").await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("user.create", "user")
                .entity(&id)
                .meta(json!({ "username": username, "projects": grants.len() })),
        )
        .await?;
        let row = users::get(&mut tx, &id).await?.ok_or_else(not_found)?;
        tx.commit().await.map_err(db_err)?;
        Ok(UserCreated {
            user: dto(row, &grants),
            invite: AccountLink {
                url,
                expires_at,
                username,
            },
        })
    }

    /// Replace the Member's whole grant set.
    pub async fn update_access(
        &self,
        actor: &Actor,
        user_id: &str,
        grants: &[GrantInput],
    ) -> Result<UserDto, CoreError> {
        actor.authorize(&OP_USER_ACCESS_UPDATE, None)?;
        let grants = normalize_grants(grants)?;
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        let row = member_target(&mut tx, user_id).await?;
        access::replace(&mut tx, user_id, &grants, now).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("user.access_update", "user")
                .entity(user_id)
                .meta(json!({
                    "projects": grants.keys().collect::<Vec<_>>(),
                    "permission_count": grants.values().map(BTreeSet::len).sum::<usize>(),
                })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(dto(row, &grants))
    }

    /// Disable (revokes sessions) or re-enable a Member.
    pub async fn set_status(
        &self,
        actor: &Actor,
        user_id: &str,
        status: UserStatus,
    ) -> Result<UserDto, CoreError> {
        actor.authorize(&OP_USER_STATUS_UPDATE, None)?;
        if status == UserStatus::Invited {
            return Err(CoreError::validation(
                "status",
                "status must be active or disabled",
            ));
        }
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        let row = member_target(&mut tx, user_id).await?;
        // Re-enabling a user who never set a password returns them to `invited`.
        let new_status = match status {
            UserStatus::Active if row.password_hash.is_none() => UserStatus::Invited,
            s => s,
        };
        users::set_status(&mut tx, user_id, new_status, now).await?;
        if new_status == UserStatus::Disabled {
            sessions::delete_for_user(&mut tx, user_id).await?;
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("user.status_update", "user")
                .entity(user_id)
                .meta(json!({ "status": new_status.as_str() })),
        )
        .await?;
        let updated = users::get(&mut tx, user_id).await?.ok_or_else(not_found)?;
        let grants = access::load(&mut tx, user_id).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(dto(updated, &grants))
    }

    /// New one-time reset link (24h). The password is cleared and all sessions die.
    pub async fn reset_link(&self, actor: &Actor, user_id: &str) -> Result<AccountLink, CoreError> {
        actor.authorize(&OP_USER_RESET_LINK, None)?;
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        let row = member_target(&mut tx, user_id).await?;
        if row.status() == UserStatus::Disabled {
            return Err(CoreError::conflict("enable the user before issuing a link"));
        }
        users::clear_password_invited(&mut tx, user_id, now).await?;
        sessions::delete_for_user(&mut tx, user_id).await?;
        account_tokens::mark_all_used(&mut tx, user_id, now).await?;
        let (url, expires_at) = issue_account_token(&self.ctx, &mut tx, user_id, "reset").await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("user.reset_link", "user").entity(user_id),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(AccountLink {
            url,
            expires_at,
            username: row.username,
        })
    }

    /// Delete a Member with their sessions, grants and API Tokens. Audit rows keep the label.
    pub async fn delete(&self, actor: &Actor, user_id: &str) -> Result<(), CoreError> {
        actor.authorize(&OP_USER_DELETE, None)?;
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        let row = member_target(&mut tx, user_id).await?;
        let tokens = api_tokens::revoke_all_for_user(&mut tx, user_id, now).await?;
        users::delete(&mut tx, user_id).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("user.delete", "user")
                .entity(user_id)
                .meta(json!({ "username": row.username, "tokens_revoked": tokens })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }
}

fn not_found() -> CoreError {
    CoreError::not_found("user not found")
}

/// The target of a Member-management operation: must exist and must not be the Owner.
async fn member_target(
    conn: &mut sqlx::SqliteConnection,
    user_id: &str,
) -> Result<UserRow, CoreError> {
    let row = users::get(conn, user_id).await?.ok_or_else(not_found)?;
    if row.role() == Role::Owner {
        return Err(CoreError::validation(
            "user",
            "the Owner cannot be changed this way (use `yanto owner-reset` or your own settings)",
        ));
    }
    Ok(row)
}

fn dto(r: UserRow, grants: &Grants) -> UserDto {
    UserDto {
        role: r.role().as_str(),
        status: r.status().as_str(),
        id: r.id,
        username: r.username,
        last_login_at: r.last_login_at,
        created_at: r.created_at,
        grants: grants
            .iter()
            .map(|(p, perms)| GrantDto {
                project_id: p.clone(),
                permissions: perms.iter().map(|x| x.as_str().to_owned()).collect(),
            })
            .collect(),
    }
}

/// Usernames: 1-64 chars of `[A-Za-z0-9._@-]`, trimmed.
pub(crate) fn validate_username(raw: &str) -> Result<String, CoreError> {
    let u = raw.trim();
    if u.is_empty() || u.len() > 64 {
        return Err(CoreError::validation(
            "username",
            "username must be 1-64 characters",
        ));
    }
    if !u
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '@' | '-'))
    {
        return Err(CoreError::validation(
            "username",
            "username may contain letters, digits and . _ @ -",
        ));
    }
    Ok(u.to_owned())
}

/// Validate Project ids and merge duplicate entries.
fn normalize_grants(input: &[GrantInput]) -> Result<Grants, CoreError> {
    let mut out: BTreeMap<String, BTreeSet<Permission>> = BTreeMap::new();
    for g in input {
        let id = g.project_id.trim();
        if id.is_empty()
            || id.len() > 64
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        {
            return Err(CoreError::validation(
                "project_id",
                "project id must be 1-64 characters of letters, digits, _ and -",
            ));
        }
        out.entry(id.to_owned())
            .or_default()
            .extend(g.permissions.iter().copied());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_rules() {
        assert_eq!(validate_username("  alice.b@x-1 ").unwrap(), "alice.b@x-1");
        assert!(validate_username("").is_err());
        assert!(validate_username("a b").is_err());
        assert!(validate_username(&"a".repeat(65)).is_err());
    }

    #[test]
    fn grants_merge_and_validate() {
        let g = normalize_grants(&[
            GrantInput {
                project_id: "prj_1".into(),
                permissions: vec![Permission::Deploy],
            },
            GrantInput {
                project_id: "prj_1".into(),
                permissions: vec![Permission::Config],
            },
        ])
        .unwrap();
        assert_eq!(g["prj_1"].len(), 2);
        assert!(
            normalize_grants(&[GrantInput {
                project_id: "bad id".into(),
                permissions: vec![]
            }])
            .is_err()
        );
    }
}
