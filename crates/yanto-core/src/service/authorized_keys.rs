//! Authorized Keys (docs/rewrite/04 section 6): SQLite is the source of truth for the marked block
//! in the configured Unix user's `~/.ssh/authorized_keys`. Everything outside the block is never
//! touched. A change that would leave the file without any key is refused (lockout guard, 6.5).

use serde::Serialize;
use serde_json::json;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use crate::actor::{Access, Actor, OpSpec, Scope};
use crate::authkeys::{self, SpliceError};
use crate::crypto::new_id;
use crate::db::repo::authorized_keys::{self as repo, KeyRow};
use crate::db::repo::db_err;
use crate::error::CoreError;
use crate::ports::{HostMgmtPorts, KeysEdit, SshdInfo};

pub const OP_AUTHORIZED_KEYS_LIST: OpSpec =
    OpSpec::new("authorized_keys_list", Access::Owner, Scope::Read);
pub const OP_AUTHORIZED_KEYS_ADD: OpSpec =
    OpSpec::new("authorized_keys_add", Access::Owner, Scope::Admin);
pub const OP_AUTHORIZED_KEYS_DELETE: OpSpec =
    OpSpec::new("authorized_keys_delete", Access::Owner, Scope::Admin);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_AUTHORIZED_KEYS_LIST,
    &OP_AUTHORIZED_KEYS_ADD,
    &OP_AUTHORIZED_KEYS_DELETE,
];

#[derive(Debug, Clone, Serialize)]
pub struct KeyDto {
    pub id: String,
    pub name: String,
    pub key_type: String,
    pub public_key: String,
    pub fingerprint: String,
    pub enabled: bool,
    pub created_at: i64,
}

impl From<KeyRow> for KeyDto {
    fn from(r: KeyRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            key_type: r.key_type,
            public_key: r.public_key,
            fingerprint: r.fingerprint,
            enabled: r.enabled,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FileState {
    Missing,
    Ok,
    /// The markers are malformed: the block is read-only until fixed by hand (6.6).
    Ambiguous {
        detail: String,
    },
    Unreadable {
        detail: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct KeysOverview {
    pub unix_user: String,
    /// Set when the configured user cannot be used (missing account, relative home): the feature is off.
    pub problem: Option<String>,
    pub file_path: Option<String>,
    pub file: FileState,
    pub keys: Vec<KeyDto>,
    /// Key lines outside the block (never edited).
    pub outside_keys: usize,
    /// The block on disk equals what the DB would render.
    pub block_in_sync: bool,
    pub sshd: Option<SshdInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AddKey {
    /// Blank derives a name from the key's comment.
    pub name: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyAdded {
    pub key: KeyDto,
    pub warnings: Vec<String>,
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyDeleted {
    pub remaining_keys: usize,
    pub forced: bool,
    pub backup_path: Option<String>,
}

#[derive(Clone)]
pub struct AuthorizedKeysService {
    ctx: Ctx,
    host: HostMgmtPorts,
    user: String,
}

impl std::fmt::Debug for AuthorizedKeysService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizedKeysService")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

fn block_for(rows: &[KeyRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| r.enabled)
        .map(|r| authkeys::managed_line(&r.id, &r.name, &r.public_key))
        .collect()
}

fn ambiguous(e: SpliceError) -> CoreError {
    CoreError::conflict(e.to_string())
}

impl AuthorizedKeysService {
    pub(super) fn new(ctx: Ctx, host: HostMgmtPorts, unix_user: String) -> Self {
        Self {
            ctx,
            host,
            user: unix_user,
        }
    }

    pub async fn list(&self, actor: &Actor) -> Result<KeysOverview, CoreError> {
        actor.authorize(&OP_AUTHORIZED_KEYS_LIST, None)?;
        let rows = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            repo::list(&mut conn).await?
        };
        let expected = block_for(&rows);
        let mut out = KeysOverview {
            unix_user: self.user.clone(),
            problem: None,
            file_path: None,
            file: FileState::Missing,
            keys: rows.into_iter().map(Into::into).collect(),
            outside_keys: 0,
            block_in_sync: expected.is_empty(),
            sshd: None,
            warnings: Vec::new(),
        };
        let unix = match self.host.authorized_keys.resolve(&self.user) {
            Ok(u) => u,
            Err(e) => {
                out.problem = Some(e.message);
                return Ok(out);
            }
        };
        out.file_path = Some(
            self.host
                .authorized_keys
                .file_path(&unix)
                .display()
                .to_string(),
        );
        match self.host.authorized_keys.read(&self.user) {
            Ok(None) => {}
            Ok(Some(bytes)) => match authkeys::outside_block(&bytes) {
                Ok(outside) => {
                    out.file = FileState::Ok;
                    out.outside_keys = authkeys::count_valid_keys(&outside);
                    out.block_in_sync =
                        authkeys::block_lines(&bytes).map_err(ambiguous)? == expected;
                }
                Err(e) => {
                    out.file = FileState::Ambiguous {
                        detail: e.to_string(),
                    };
                    out.block_in_sync = false;
                }
            },
            Err(e) => {
                out.file = FileState::Unreadable { detail: e.message };
                out.block_in_sync = false;
            }
        }
        out.sshd = self.host.authorized_keys.sshd_info();
        if let Some(s) = &out.sshd {
            if s.pubkey_authentication == Some(false) {
                out.warnings.push(
                    "sshd has public key authentication disabled: these keys have no effect".into(),
                );
            }
            if !s.authorized_keys_files.is_empty()
                && !s.authorized_keys_files.iter().any(|f| {
                    f == ".ssh/authorized_keys"
                        || f.ends_with("/.ssh/authorized_keys")
                        || f.contains('%')
                })
            {
                out.warnings
                    .push("sshd's AuthorizedKeysFile does not include .ssh/authorized_keys".into());
            }
        }
        Ok(out)
    }

    pub async fn add(&self, actor: &Actor, input: &AddKey) -> Result<KeyAdded, CoreError> {
        actor.authorize(&OP_AUTHORIZED_KEYS_ADD, None)?;
        let parsed = authkeys::parse_public_key(&input.public_key)?;
        let name = authkeys::resolve_name(&input.name, &parsed.comment)?;
        self.host.authorized_keys.resolve(&self.user)?;

        let row = KeyRow {
            id: new_id("akey"),
            name,
            key_type: parsed.key_type.clone(),
            public_key: parsed.public_key.clone(),
            fingerprint: parsed.fingerprint.clone(),
            enabled: true,
            created_at: self.ctx.now(),
        };
        let mut tx = self.ctx.db.begin_write().await?;
        repo::insert(&mut tx, &row).await?;
        let block = block_for(&repo::list(&mut tx).await?);

        let mut warnings: Vec<String> = parsed.warning.into_iter().collect();
        let fingerprint = parsed.fingerprint;
        let mut outside_dup = false;
        let updated = self.host.authorized_keys.update(&self.user, &mut |bytes| {
            let spliced = authkeys::splice(bytes, &block).map_err(ambiguous)?;
            outside_dup =
                authkeys::outside_fingerprints(&spliced.expected_outside).contains(&fingerprint);
            Ok(Some(KeysEdit {
                bytes: spliced.bytes,
                expected_outside: spliced.expected_outside,
            }))
        })?;
        if outside_dup {
            warnings
                .push("this key is also present outside the Yanto block (left untouched)".into());
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("authorized_key.add", "authorized_key")
                .entity(&row.id)
                .meta(json!({ "name": row.name, "fingerprint": row.fingerprint,
                    "key_type": row.key_type, "unix_user": self.user })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(KeyAdded {
            key: row.into(),
            warnings,
            backup_path: updated.backup_path.map(|p| p.display().to_string()),
        })
    }

    /// Remove a key. Removing the last key anywhere in the file is refused unless
    /// `confirm_user` equals the Unix user's name (audited as `authorized_key.force_change`).
    pub async fn delete(
        &self,
        actor: &Actor,
        id: &str,
        confirm_user: Option<&str>,
    ) -> Result<KeyDeleted, CoreError> {
        actor.authorize(&OP_AUTHORIZED_KEYS_DELETE, None)?;
        self.host.authorized_keys.resolve(&self.user)?;
        let mut tx = self.ctx.db.begin_write().await?;
        let Some(row) = repo::get(&mut tx, id).await? else {
            return Err(CoreError::not_found("key not found"));
        };
        repo::delete(&mut tx, id).await?;
        let block = block_for(&repo::list(&mut tx).await?);
        let force = confirm_user == Some(self.user.as_str());
        let user = self.user.clone();

        let mut remaining = 0usize;
        let mut forced = false;
        let updated = self.host.authorized_keys.update(&self.user, &mut |bytes| {
            let spliced = authkeys::splice(bytes, &block).map_err(ambiguous)?;
            remaining = authkeys::count_valid_keys(&spliced.bytes);
            if remaining == 0 {
                if !force {
                    return Err(CoreError::conflict(format!(
                        "lockout guard (Rule 1): this would leave {user}'s authorized_keys with no key at all. \
                         Add another key first, or confirm by typing the Unix user name"
                    )));
                }
                forced = true;
            }
            Ok(Some(KeysEdit {
                bytes: spliced.bytes,
                expected_outside: spliced.expected_outside,
            }))
        })?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new(
                if forced {
                    "authorized_key.force_change"
                } else {
                    "authorized_key.delete"
                },
                "authorized_key",
            )
            .entity(id)
            .meta(json!({ "name": row.name, "fingerprint": row.fingerprint,
                "unix_user": self.user, "remaining_keys": remaining })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(KeyDeleted {
            remaining_keys: remaining,
            forced,
            backup_path: updated.backup_path.map(|p| p.display().to_string()),
        })
    }
}
