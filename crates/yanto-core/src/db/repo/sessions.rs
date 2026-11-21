//! `sessions` table. Ids are the SHA-256 hex of the cookie value.

use sqlx::{FromRow, SqliteConnection};

use super::db_err;
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct SessionRow {
    pub id: String,
    pub user_id: String,
    pub csrf_token: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_seen_at: i64,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

pub struct NewSession<'a> {
    pub id: &'a str,
    pub user_id: &'a str,
    pub csrf_token: &'a str,
    pub now: i64,
    pub expires_at: i64,
    pub ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
}

pub async fn insert(conn: &mut SqliteConnection, s: &NewSession<'_>) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO sessions (id, user_id, csrf_token, created_at, expires_at, last_seen_at, ip, user_agent)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(s.id)
    .bind(s.user_id)
    .bind(s.csrf_token)
    .bind(s.now)
    .bind(s.expires_at)
    .bind(s.now)
    .bind(s.ip)
    .bind(s.user_agent)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn get(conn: &mut SqliteConnection, id: &str) -> Result<Option<SessionRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, user_id, csrf_token, created_at, expires_at, last_seen_at, ip, user_agent
         FROM sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

pub async fn touch(conn: &mut SqliteConnection, id: &str, now: i64) -> Result<(), CoreError> {
    sqlx::query("UPDATE sessions SET last_seen_at = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn delete(conn: &mut SqliteConnection, id: &str) -> Result<bool, CoreError> {
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected() > 0)
        .map_err(db_err)
}

/// Delete one session only if it belongs to `user_id`.
pub async fn delete_owned(
    conn: &mut SqliteConnection,
    id: &str,
    user_id: &str,
) -> Result<bool, CoreError> {
    sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected() > 0)
        .map_err(db_err)
}

pub async fn delete_for_user(conn: &mut SqliteConnection, user_id: &str) -> Result<u64, CoreError> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected())
        .map_err(db_err)
}

/// Delete every session of `user_id` except `keep`.
pub async fn delete_for_user_except(
    conn: &mut SqliteConnection,
    user_id: &str,
    keep: &str,
) -> Result<u64, CoreError> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id <> ?")
        .bind(user_id)
        .bind(keep)
        .execute(conn)
        .await
        .map(|r| r.rows_affected())
        .map_err(db_err)
}

pub async fn list_for_user(
    conn: &mut SqliteConnection,
    user_id: &str,
) -> Result<Vec<SessionRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, user_id, csrf_token, created_at, expires_at, last_seen_at, ip, user_agent
         FROM sessions WHERE user_id = ? ORDER BY last_seen_at DESC",
    )
    .bind(user_id)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

/// Remove sessions past their absolute expiry or idle for longer than `idle_cutoff`.
pub async fn delete_expired(
    conn: &mut SqliteConnection,
    now: i64,
    idle_cutoff: i64,
) -> Result<u64, CoreError> {
    sqlx::query("DELETE FROM sessions WHERE expires_at <= ? OR last_seen_at <= ?")
        .bind(now)
        .bind(idle_cutoff)
        .execute(conn)
        .await
        .map(|r| r.rows_affected())
        .map_err(db_err)
}
