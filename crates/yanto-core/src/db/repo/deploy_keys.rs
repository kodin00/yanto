//! `deploy_keys` table.

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection};

use super::{db_err, is_unique_violation};
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct DeployKeyRow {
    pub id: String,
    pub name: String,
    pub key_type: String,
    pub public_key: String,
    pub fingerprint: String,
    pub private_key_enc: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct DeployKeyPublic {
    pub id: String,
    pub name: String,
    pub key_type: String,
    pub public_key: String,
    pub fingerprint: String,
    pub created_at: i64,
}

pub async fn insert(conn: &mut SqliteConnection, k: &DeployKeyRow) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO deploy_keys (id, name, key_type, public_key, fingerprint, private_key_enc, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&k.id)
    .bind(&k.name)
    .bind(&k.key_type)
    .bind(&k.public_key)
    .bind(&k.fingerprint)
    .bind(&k.private_key_enc)
    .bind(k.created_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("a deploy key with this name or fingerprint already exists")
        } else {
            db_err(e)
        }
    })
}

pub async fn list(conn: &mut SqliteConnection) -> Result<Vec<DeployKeyPublic>, CoreError> {
    sqlx::query_as(
        "SELECT id, name, key_type, public_key, fingerprint, created_at FROM deploy_keys ORDER BY name",
    )
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn get_public(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<DeployKeyPublic>, CoreError> {
    sqlx::query_as(
        "SELECT id, name, key_type, public_key, fingerprint, created_at FROM deploy_keys WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

pub async fn get(conn: &mut SqliteConnection, id: &str) -> Result<Option<DeployKeyRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, name, key_type, public_key, fingerprint, private_key_enc, created_at
         FROM deploy_keys WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

pub async fn delete(conn: &mut SqliteConnection, id: &str) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM deploy_keys WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}
