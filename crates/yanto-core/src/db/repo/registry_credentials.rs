//! `registry_credentials` table (pull-side credentials for external registries).

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection};

use super::{db_err, is_unique_violation};
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct CredentialRow {
    pub id: String,
    pub name: String,
    pub registry_host: String,
    pub username: String,
    pub secret_enc: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// What every adapter may see: never the secret.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct CredentialPublic {
    pub id: String,
    pub name: String,
    pub registry_host: String,
    pub username: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub async fn insert(conn: &mut SqliteConnection, c: &CredentialRow) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO registry_credentials (id, name, registry_host, username, secret_enc, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&c.id)
    .bind(&c.name)
    .bind(&c.registry_host)
    .bind(&c.username)
    .bind(&c.secret_enc)
    .bind(c.created_at)
    .bind(c.updated_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("a registry credential with this name already exists")
        } else {
            db_err(e)
        }
    })
}

pub async fn save(conn: &mut SqliteConnection, c: &CredentialRow) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE registry_credentials SET name = ?, registry_host = ?, username = ?, secret_enc = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&c.name)
    .bind(&c.registry_host)
    .bind(&c.username)
    .bind(&c.secret_enc)
    .bind(c.updated_at)
    .bind(&c.id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("a registry credential with this name already exists")
        } else {
            db_err(e)
        }
    })
}

pub async fn get(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<CredentialRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, name, registry_host, username, secret_enc, created_at, updated_at
         FROM registry_credentials WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

pub async fn list(conn: &mut SqliteConnection) -> Result<Vec<CredentialPublic>, CoreError> {
    sqlx::query_as(
        "SELECT id, name, registry_host, username, created_at, updated_at
         FROM registry_credentials ORDER BY name",
    )
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn delete(conn: &mut SqliteConnection, id: &str) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM registry_credentials WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}
