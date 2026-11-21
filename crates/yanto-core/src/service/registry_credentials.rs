//! Registry Credentials for pulling private external images (docs/rewrite/04 4.7).
//! The secret is write-only in every adapter: sealed at rest, never returned, and used
//! only through a per-command temporary `DOCKER_CONFIG`.

use serde_json::json;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use crate::actor::{Access, Actor, OpSpec, Scope};
use crate::crypto::new_id;
use crate::db::repo::registry_credentials::{self, CredentialPublic, CredentialRow};
use crate::db::repo::{db_err, projects};
use crate::error::CoreError;
use crate::secrets::Secret;

pub const OP_REGISTRY_CREDENTIAL_LIST: OpSpec = OpSpec::new(
    "registry_credential_list",
    Access::Authenticated,
    Scope::Read,
);
pub const OP_REGISTRY_CREDENTIAL_CREATE: OpSpec =
    OpSpec::new("registry_credential_create", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_CREDENTIAL_UPDATE: OpSpec =
    OpSpec::new("registry_credential_update", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_CREDENTIAL_DELETE: OpSpec =
    OpSpec::new("registry_credential_delete", Access::Owner, Scope::Admin);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_REGISTRY_CREDENTIAL_LIST,
    &OP_REGISTRY_CREDENTIAL_CREATE,
    &OP_REGISTRY_CREDENTIAL_UPDATE,
    &OP_REGISTRY_CREDENTIAL_DELETE,
];

#[derive(Debug, Clone, Default)]
pub struct CredentialUpdate {
    pub name: Option<String>,
    pub registry_host: Option<String>,
    pub username: Option<String>,
    /// Rotate the secret when present.
    pub secret: Option<Secret>,
}

#[derive(Debug, Clone)]
pub struct RegistryCredentialService {
    ctx: Ctx,
}

/// `https://GHCR.io/` becomes `ghcr.io`; letters, digits, `.`, `-`, `:` only.
pub(crate) fn normalize_host(raw: &str) -> Result<String, CoreError> {
    let h = raw
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    let ok = !h.is_empty()
        && h.len() <= 255
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
        && !h.starts_with('-');
    if ok {
        Ok(h)
    } else {
        Err(CoreError::validation(
            "registry_host",
            "registry host must look like ghcr.io, docker.io or host:port",
        ))
    }
}

fn required(field: &str, v: &str, max: usize) -> Result<String, CoreError> {
    let v = v.trim();
    if v.is_empty() || v.chars().count() > max || v.chars().any(char::is_control) {
        return Err(CoreError::validation(
            field,
            format!("{field} must be 1-{max} characters"),
        ));
    }
    Ok(v.to_owned())
}

fn check_secret(s: &Secret) -> Result<(), CoreError> {
    let v = s.expose();
    if v.is_empty() || v.len() > 4096 || v.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "secret",
            "secret must be 1-4096 characters on one line",
        ));
    }
    Ok(())
}

impl RegistryCredentialService {
    pub(super) fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    pub async fn list(&self, actor: &Actor) -> Result<Vec<CredentialPublic>, CoreError> {
        actor.authorize(&OP_REGISTRY_CREDENTIAL_LIST, None)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        registry_credentials::list(&mut conn).await
    }

    pub async fn create(
        &self,
        actor: &Actor,
        name: &str,
        registry_host: &str,
        username: &str,
        secret: &Secret,
    ) -> Result<CredentialPublic, CoreError> {
        actor.authorize(&OP_REGISTRY_CREDENTIAL_CREATE, None)?;
        let name = required("name", name, 100)?;
        let host = normalize_host(registry_host)?;
        let username = required("username", username, 255)?;
        check_secret(secret)?;
        let id = new_id("rcr");
        let now = self.ctx.now();
        let row = CredentialRow {
            secret_enc: self
                .ctx
                .secrets
                .seal("registry_credentials", "secret_enc", &id, secret)?,
            id: id.clone(),
            name,
            registry_host: host,
            username,
            created_at: now,
            updated_at: now,
        };
        let mut tx = self.ctx.db.begin_write().await?;
        registry_credentials::insert(&mut tx, &row).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("registry_credential.create", "registry_credential")
                .entity(&id)
                .meta(json!({ "name": row.name, "registry_host": row.registry_host })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(public(&row))
    }

    pub async fn update(
        &self,
        actor: &Actor,
        id: &str,
        change: &CredentialUpdate,
    ) -> Result<CredentialPublic, CoreError> {
        actor.authorize(&OP_REGISTRY_CREDENTIAL_UPDATE, None)?;
        let mut tx = self.ctx.db.begin_write().await?;
        let mut row = registry_credentials::get(&mut tx, id)
            .await?
            .ok_or_else(|| CoreError::not_found("registry credential not found"))?;
        let mut changed = Vec::new();
        if let Some(n) = &change.name {
            row.name = required("name", n, 100)?;
            changed.push("name");
        }
        if let Some(h) = &change.registry_host {
            row.registry_host = normalize_host(h)?;
            changed.push("registry_host");
        }
        if let Some(u) = &change.username {
            row.username = required("username", u, 255)?;
            changed.push("username");
        }
        if let Some(s) = &change.secret {
            check_secret(s)?;
            row.secret_enc =
                self.ctx
                    .secrets
                    .seal("registry_credentials", "secret_enc", &row.id, s)?;
            changed.push("secret");
        }
        row.updated_at = self.ctx.now();
        registry_credentials::save(&mut tx, &row).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("registry_credential.update", "registry_credential")
                .entity(id)
                .meta(json!({ "changed": changed })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(public(&row))
    }

    /// 409 (naming the Projects) while any Project still references it.
    pub async fn delete(&self, actor: &Actor, id: &str) -> Result<(), CoreError> {
        actor.authorize(&OP_REGISTRY_CREDENTIAL_DELETE, None)?;
        let mut tx = self.ctx.db.begin_write().await?;
        let row = registry_credentials::get(&mut tx, id)
            .await?
            .ok_or_else(|| CoreError::not_found("registry credential not found"))?;
        let users = projects::names_for_registry_credential(&mut tx, id).await?;
        if !users.is_empty() {
            return Err(CoreError::conflict(format!(
                "registry credential is used by: {}",
                users.join(", ")
            )));
        }
        registry_credentials::delete(&mut tx, id).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("registry_credential.delete", "registry_credential")
                .entity(id)
                .meta(json!({ "name": row.name })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }
}

fn public(r: &CredentialRow) -> CredentialPublic {
    CredentialPublic {
        id: r.id.clone(),
        name: r.name.clone(),
        registry_host: r.registry_host.clone(),
        username: r.username.clone(),
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_normalize() {
        assert_eq!(normalize_host("https://GHCR.io/").unwrap(), "ghcr.io");
        assert_eq!(normalize_host("127.0.0.1:5000").unwrap(), "127.0.0.1:5000");
        assert!(normalize_host("bad host").is_err());
        assert!(normalize_host("").is_err());
    }
}
