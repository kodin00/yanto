//! `app_settings` key/value rows (values are JSON text).

use sqlx::SqliteConnection;

use super::db_err;
use crate::error::CoreError;

pub async fn get(conn: &mut SqliteConnection, key: &str) -> Result<Option<String>, CoreError> {
    sqlx::query_scalar("SELECT value FROM app_settings WHERE key = ?")
        .bind(key)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

/// Upsert a JSON value.
pub async fn set(
    conn: &mut SqliteConnection,
    key: &str,
    json: &str,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO app_settings (key, value, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(json)
    .bind(now)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn delete(conn: &mut SqliteConnection, key: &str) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM app_settings WHERE key = ?")
        .bind(key)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}
