//! `authorized_keys` table.

use sqlx::{FromRow, SqliteConnection};

use super::{db_err, is_unique_violation};
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct KeyRow {
    pub id: String,
    pub name: String,
    pub key_type: String,
    pub public_key: String,
    pub fingerprint: String,
    pub enabled: bool,
    pub created_at: i64,
}

const SELECT: &str =
    "SELECT id, name, key_type, public_key, fingerprint, enabled, created_at FROM authorized_keys";

pub async fn insert(conn: &mut SqliteConnection, k: &KeyRow) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO authorized_keys (id, name, key_type, public_key, fingerprint, enabled, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&k.id)
    .bind(&k.name)
    .bind(&k.key_type)
    .bind(&k.public_key)
    .bind(&k.fingerprint)
    .bind(k.enabled)
    .bind(k.created_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("that key (or name) is already present")
        } else {
            db_err(e)
        }
    })
}

pub async fn delete(conn: &mut SqliteConnection, id: &str) -> Result<bool, CoreError> {
    sqlx::query("DELETE FROM authorized_keys WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected() > 0)
        .map_err(db_err)
}

pub async fn get(conn: &mut SqliteConnection, id: &str) -> Result<Option<KeyRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = ?")))
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn list(conn: &mut SqliteConnection) -> Result<Vec<KeyRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY created_at, id"
    )))
    .fetch_all(conn)
    .await
    .map_err(db_err)
}
