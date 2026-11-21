//! Deploy Keys: SSH keys Yanto uses to clone private repositories (docs/rewrite/04 1.3).
//! Listing shows public data only. Creating, importing and deleting are Owner-only; the
//! private half is write-only in every adapter and stored AES-GCM sealed.

use serde_json::json;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use crate::actor::{Access, Actor, OpSpec, Scope};
use crate::crypto::new_id;
use crate::db::repo::deploy_keys::{self, DeployKeyPublic, DeployKeyRow};
use crate::db::repo::{db_err, projects};
use crate::deploy::keys;
use crate::error::CoreError;
use crate::secrets::Secret;

pub const OP_DEPLOY_KEY_LIST: OpSpec =
    OpSpec::new("deploy_key_list", Access::Authenticated, Scope::Read);
pub const OP_DEPLOY_KEY_PUBLIC_GET: OpSpec =
    OpSpec::new("deploy_key_public_get", Access::Authenticated, Scope::Read);
pub const OP_DEPLOY_KEY_CREATE: OpSpec =
    OpSpec::new("deploy_key_create", Access::Owner, Scope::Admin);
pub const OP_DEPLOY_KEY_IMPORT: OpSpec =
    OpSpec::new("deploy_key_import", Access::Owner, Scope::Admin);
pub const OP_DEPLOY_KEY_DELETE: OpSpec =
    OpSpec::new("deploy_key_delete", Access::Owner, Scope::Admin);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_DEPLOY_KEY_LIST,
    &OP_DEPLOY_KEY_PUBLIC_GET,
    &OP_DEPLOY_KEY_CREATE,
    &OP_DEPLOY_KEY_IMPORT,
    &OP_DEPLOY_KEY_DELETE,
];

#[derive(Debug, Clone)]
pub struct DeployKeyService {
    ctx: Ctx,
}

pub(crate) fn key_name(raw: &str) -> Result<String, CoreError> {
    let n = raw.trim();
    if n.is_empty() || n.chars().count() > 100 || n.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "name",
            "name must be 1-100 characters",
        ));
    }
    Ok(n.to_owned())
}

/// Build the row (id, sealed private key) for a new key.
pub(crate) fn key_row(
    ctx: &Ctx,
    name: &str,
    key: keys::GeneratedKey,
) -> Result<DeployKeyRow, CoreError> {
    let id = new_id("dky");
    let enc = ctx
        .secrets
        .seal("deploy_keys", "private_key_enc", &id, &key.private_openssh)?;
    Ok(DeployKeyRow {
        id,
        name: name.to_owned(),
        key_type: "ed25519".into(),
        public_key: key.public_key,
        fingerprint: key.fingerprint,
        private_key_enc: enc,
        created_at: ctx.now(),
    })
}

impl DeployKeyService {
    pub(super) fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    pub async fn list(&self, actor: &Actor) -> Result<Vec<DeployKeyPublic>, CoreError> {
        actor.authorize(&OP_DEPLOY_KEY_LIST, None)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        deploy_keys::list(&mut conn).await
    }

    pub async fn public_get(&self, actor: &Actor, id: &str) -> Result<DeployKeyPublic, CoreError> {
        actor.authorize(&OP_DEPLOY_KEY_PUBLIC_GET, None)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        deploy_keys::get_public(&mut conn, id)
            .await?
            .ok_or_else(|| CoreError::not_found("deploy key not found"))
    }

    /// Generate an ed25519 key pair in-process.
    pub async fn create(&self, actor: &Actor, name: &str) -> Result<DeployKeyPublic, CoreError> {
        actor.authorize(&OP_DEPLOY_KEY_CREATE, None)?;
        let name = key_name(name)?;
        let generated = keys::generate_ed25519(&format!("yanto:{name}"))?;
        self.store(actor, &name, generated, "deploy_key.create")
            .await
    }

    /// Import an existing unencrypted OpenSSH ed25519 private key (write-only).
    pub async fn import(
        &self,
        actor: &Actor,
        name: &str,
        private_key: &Secret,
    ) -> Result<DeployKeyPublic, CoreError> {
        actor.authorize(&OP_DEPLOY_KEY_IMPORT, None)?;
        let name = key_name(name)?;
        let parsed = keys::import_openssh(private_key, &format!("yanto:{name}"))?;
        self.store(actor, &name, parsed, "deploy_key.import").await
    }

    async fn store(
        &self,
        actor: &Actor,
        name: &str,
        key: keys::GeneratedKey,
        action: &'static str,
    ) -> Result<DeployKeyPublic, CoreError> {
        let row = key_row(&self.ctx, name, key)?;
        let id = row.id.clone();
        let mut tx = self.ctx.db.begin_write().await?;
        deploy_keys::insert(&mut tx, &row).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new(action, "deploy_key")
                .entity(&id)
                .meta(json!({ "name": name, "fingerprint": row.fingerprint })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(DeployKeyPublic {
            id,
            name: row.name,
            key_type: row.key_type,
            public_key: row.public_key,
            fingerprint: row.fingerprint,
            created_at: row.created_at,
        })
    }

    /// 409 (naming the Projects) while any Project still uses the key.
    pub async fn delete(&self, actor: &Actor, id: &str) -> Result<(), CoreError> {
        actor.authorize(&OP_DEPLOY_KEY_DELETE, None)?;
        let mut tx = self.ctx.db.begin_write().await?;
        let key = deploy_keys::get_public(&mut tx, id)
            .await?
            .ok_or_else(|| CoreError::not_found("deploy key not found"))?;
        let users = projects::count_for_deploy_key(&mut tx, id).await?;
        if !users.is_empty() {
            return Err(CoreError::conflict(format!(
                "deploy key is used by: {}",
                users.join(", ")
            )));
        }
        deploy_keys::delete(&mut tx, id).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("deploy_key.delete", "deploy_key")
                .entity(id)
                .meta(json!({ "name": key.name })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }
}
