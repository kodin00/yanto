//! `hosted_registry_settings` (singleton) and `registry_users`.

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection};

use super::{db_err, is_unique_violation};
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct SettingsRow {
    pub enabled: bool,
    pub listen_port: i64,
    pub container_name: String,
    pub image_digest: Option<String>,
    pub gc_in_progress: bool,
    pub internal_password_enc: Option<String>,
    pub public_host: Option<String>,
    pub updated_at: i64,
}

const SETTINGS_COLS: &str = "enabled, listen_port, container_name, image_digest, gc_in_progress, internal_password_enc, public_host, updated_at";

pub async fn settings(conn: &mut SqliteConnection) -> Result<SettingsRow, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SETTINGS_COLS} FROM hosted_registry_settings WHERE id = 1"
    )))
    .fetch_one(conn)
    .await
    .map_err(db_err)
}

pub async fn save_settings(conn: &mut SqliteConnection, s: &SettingsRow) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE hosted_registry_settings SET enabled = ?, listen_port = ?, container_name = ?,
            image_digest = ?, gc_in_progress = ?, internal_password_enc = ?, public_host = ?, updated_at = ?
         WHERE id = 1",
    )
    .bind(s.enabled)
    .bind(s.listen_port)
    .bind(&s.container_name)
    .bind(&s.image_digest)
    .bind(s.gc_in_progress)
    .bind(&s.internal_password_enc)
    .bind(&s.public_host)
    .bind(s.updated_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

/// A registry user including the hash (internal use only; adapters get [`RegistryUserPublic`]).
#[derive(Debug, Clone, FromRow)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryUserPublic {
    pub id: String,
    pub username: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&UserRow> for RegistryUserPublic {
    fn from(r: &UserRow) -> Self {
        Self {
            id: r.id.clone(),
            username: r.username.clone(),
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub async fn users(conn: &mut SqliteConnection) -> Result<Vec<UserRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, username, password_hash, created_at, updated_at FROM registry_users
         ORDER BY username COLLATE NOCASE",
    )
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn user(conn: &mut SqliteConnection, id: &str) -> Result<Option<UserRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, username, password_hash, created_at, updated_at FROM registry_users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

pub async fn insert_user(conn: &mut SqliteConnection, u: &UserRow) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO registry_users (id, username, password_hash, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&u.id)
    .bind(&u.username)
    .bind(&u.password_hash)
    .bind(u.created_at)
    .bind(u.updated_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("a registry user with this name already exists")
        } else {
            db_err(e)
        }
    })
}

pub async fn set_user_hash(
    conn: &mut SqliteConnection,
    id: &str,
    hash: &str,
    now: i64,
) -> Result<bool, CoreError> {
    sqlx::query("UPDATE registry_users SET password_hash = ?, updated_at = ? WHERE id = ?")
        .bind(hash)
        .bind(now)
        .bind(id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected() > 0)
        .map_err(db_err)
}

pub async fn delete_user(conn: &mut SqliteConnection, id: &str) -> Result<bool, CoreError> {
    sqlx::query("DELETE FROM registry_users WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected() > 0)
        .map_err(db_err)
}
