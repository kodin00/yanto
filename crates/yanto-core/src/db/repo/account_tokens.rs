//! `account_tokens`: single-use invite/reset tokens (hash only).

use sqlx::{FromRow, SqliteConnection};

use super::db_err;
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct AccountTokenRow {
    pub id: String,
    pub user_id: String,
    pub purpose: String,
    pub expires_at: i64,
    pub used_at: Option<i64>,
}

pub async fn insert(
    conn: &mut SqliteConnection,
    id: &str,
    user_id: &str,
    purpose: &str,
    token_hash: &str,
    expires_at: i64,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO account_tokens (id, user_id, purpose, token_hash, expires_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(user_id)
    .bind(purpose)
    .bind(token_hash)
    .bind(expires_at)
    .bind(now)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn by_hash(
    conn: &mut SqliteConnection,
    token_hash: &str,
) -> Result<Option<AccountTokenRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, user_id, purpose, expires_at, used_at FROM account_tokens WHERE token_hash = ?",
    )
    .bind(token_hash)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

/// Mark every unused token of `user_id` used (completing setup, or superseding
/// older links when a new one is issued). Returns the number of rows changed.
pub async fn mark_all_used(
    conn: &mut SqliteConnection,
    user_id: &str,
    now: i64,
) -> Result<u64, CoreError> {
    sqlx::query("UPDATE account_tokens SET used_at = ? WHERE user_id = ? AND used_at IS NULL")
        .bind(now)
        .bind(user_id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected())
        .map_err(db_err)
}

/// Atomically claim a token: `true` only for the caller that flips `used_at`.
pub async fn claim(conn: &mut SqliteConnection, id: &str, now: i64) -> Result<bool, CoreError> {
    sqlx::query(
        "UPDATE account_tokens SET used_at = ? WHERE id = ? AND used_at IS NULL AND expires_at > ?",
    )
    .bind(now)
    .bind(id)
    .bind(now)
    .execute(conn)
    .await
    .map(|r| r.rows_affected() == 1)
    .map_err(db_err)
}

pub async fn delete_expired(conn: &mut SqliteConnection, now: i64) -> Result<u64, CoreError> {
    sqlx::query("DELETE FROM account_tokens WHERE expires_at <= ?")
        .bind(now)
        .execute(conn)
        .await
        .map(|r| r.rows_affected())
        .map_err(db_err)
}
