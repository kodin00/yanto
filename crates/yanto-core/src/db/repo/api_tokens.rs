//! `api_tokens` table (hash only).

use sqlx::{FromRow, SqliteConnection};

use super::{db_err, is_unique_violation};
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct ApiTokenRow {
    pub id: String,
    pub name: String,
    pub token_hint: String,
    pub scope: String,
    pub user_id: String,
    pub username: String,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub last_used_ip: Option<String>,
    pub revoked_at: Option<i64>,
    pub created_at: i64,
}

const SELECT: &str = "SELECT t.id, t.name, t.token_hint, t.scope, t.user_id, u.username,
        t.expires_at, t.last_used_at, t.last_used_ip, t.revoked_at, t.created_at
    FROM api_tokens t JOIN users u ON u.id = t.user_id";

pub struct NewToken<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub token_hash: &'a str,
    pub token_hint: &'a str,
    pub scope: &'a str,
    pub user_id: &'a str,
    pub expires_at: Option<i64>,
    pub now: i64,
}

pub async fn insert(conn: &mut SqliteConnection, t: &NewToken<'_>) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO api_tokens (id, name, token_hash, token_hint, scope, user_id, expires_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(t.id)
    .bind(t.name)
    .bind(t.token_hash)
    .bind(t.token_hint)
    .bind(t.scope)
    .bind(t.user_id)
    .bind(t.expires_at)
    .bind(t.now)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("you already have an active token with that name")
        } else {
            db_err(e)
        }
    })
}

/// A usable token (not revoked, not expired, owner `active`) by its hash.
pub async fn find_active_by_hash(
    conn: &mut SqliteConnection,
    token_hash: &str,
    now: i64,
) -> Result<Option<ApiTokenRow>, CoreError> {
    let sql = format!(
        "{SELECT} WHERE t.token_hash = ? AND t.revoked_at IS NULL
           AND (t.expires_at IS NULL OR t.expires_at > ?) AND u.status = 'active'"
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(token_hash)
        .bind(now)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn get(conn: &mut SqliteConnection, id: &str) -> Result<Option<ApiTokenRow>, CoreError> {
    let sql = format!("{SELECT} WHERE t.id = ?");
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn list_all(conn: &mut SqliteConnection) -> Result<Vec<ApiTokenRow>, CoreError> {
    let sql = format!("{SELECT} ORDER BY t.created_at DESC, t.id");
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(conn)
        .await
        .map_err(db_err)
}

pub async fn list_for_user(
    conn: &mut SqliteConnection,
    user_id: &str,
) -> Result<Vec<ApiTokenRow>, CoreError> {
    let sql = format!("{SELECT} WHERE t.user_id = ? ORDER BY t.created_at DESC, t.id");
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .fetch_all(conn)
        .await
        .map_err(db_err)
}

pub async fn touch_last_used(
    conn: &mut SqliteConnection,
    id: &str,
    now: i64,
    ip: Option<&str>,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE api_tokens SET last_used_at = ?, last_used_ip = ? WHERE id = ?")
        .bind(now)
        .bind(ip)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

/// Soft-revoke; `false` when already revoked or missing.
pub async fn revoke(conn: &mut SqliteConnection, id: &str, now: i64) -> Result<bool, CoreError> {
    sqlx::query("UPDATE api_tokens SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(now)
        .bind(id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(db_err)
}

pub async fn revoke_all_for_user(
    conn: &mut SqliteConnection,
    user_id: &str,
    now: i64,
) -> Result<u64, CoreError> {
    sqlx::query("UPDATE api_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
        .bind(now)
        .bind(user_id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected())
        .map_err(db_err)
}
