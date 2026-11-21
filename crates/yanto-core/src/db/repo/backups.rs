//! `backups` table.

use sqlx::{FromRow, SqliteConnection};

use super::db_err;
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct BackupRow {
    pub id: String,
    pub project_id: Option<String>,
    pub kind: String,
    pub database_id: Option<String>,
    pub status: String,
    pub trigger: String,
    pub filename: String,
    pub file_path: String,
    pub size_bytes: Option<i64>,
    pub sha256: Option<String>,
    pub error: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub downloaded_at: Option<i64>,
    pub download_count: i64,
}

const SELECT: &str =
    "SELECT id, project_id, kind, database_id, status, trigger, filename, file_path,
    size_bytes, sha256, error, note, created_at, finished_at, downloaded_at, download_count
    FROM backups";

pub struct NewBackup<'a> {
    pub id: &'a str,
    pub project_id: Option<&'a str>,
    pub kind: &'a str,
    pub database_id: Option<&'a str>,
    pub trigger: &'a str,
    pub filename: &'a str,
    pub file_path: &'a str,
    pub note: Option<&'a str>,
    pub now: i64,
}

pub async fn insert_running(
    conn: &mut SqliteConnection,
    b: &NewBackup<'_>,
) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO backups (id, project_id, kind, database_id, status, trigger, filename, file_path,
            note, created_at) VALUES (?, ?, ?, ?, 'running', ?, ?, ?, ?, ?)",
    )
    .bind(b.id)
    .bind(b.project_id)
    .bind(b.kind)
    .bind(b.database_id)
    .bind(b.trigger)
    .bind(b.filename)
    .bind(b.file_path)
    .bind(b.note)
    .bind(b.now)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn mark_success(
    conn: &mut SqliteConnection,
    id: &str,
    size: i64,
    sha256: &str,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE backups SET status = 'success', size_bytes = ?, sha256 = ?, finished_at = ? WHERE id = ?",
    )
    .bind(size)
    .bind(sha256)
    .bind(now)
    .bind(id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn mark_failed(
    conn: &mut SqliteConnection,
    id: &str,
    error: &str,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE backups SET status = 'failed', error = ?, finished_at = ? WHERE id = ?")
        .bind(error)
        .bind(now)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

/// Startup recovery: stale `running` rows become `failed` ("interrupted"). Returns their rows.
pub async fn fail_running(
    conn: &mut SqliteConnection,
    now: i64,
) -> Result<Vec<BackupRow>, CoreError> {
    let rows: Vec<BackupRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE status = 'running'"
    )))
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    sqlx::query(
        "UPDATE backups SET status = 'failed', error = 'interrupted', finished_at = ? WHERE status = 'running'",
    )
    .bind(now)
    .execute(conn)
    .await
    .map_err(db_err)?;
    Ok(rows)
}

pub async fn get(conn: &mut SqliteConnection, id: &str) -> Result<Option<BackupRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = ?")))
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn list(conn: &mut SqliteConnection, limit: i64) -> Result<Vec<BackupRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY created_at DESC, id DESC LIMIT ?"
    )))
    .bind(limit)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

/// Newest successful row of a kind (for the free-space guard).
pub async fn latest_success(
    conn: &mut SqliteConnection,
    kind: &str,
) -> Result<Option<BackupRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE kind = ? AND status = 'success' ORDER BY created_at DESC, id DESC LIMIT 1"
    )))
    .bind(kind)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

/// One retention series: rows of the same kind for the same Project and target (a declared
/// database, or a volume recorded in `note`). `None` fields match `NULL`.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct SeriesKey {
    pub kind: String,
    pub project_id: Option<String>,
    pub database_id: Option<String>,
    pub note: Option<String>,
}

/// Successful rows of `series` made by `trigger` (`cron` or `pre_restore`), newest first.
pub async fn series(
    conn: &mut SqliteConnection,
    key: &SeriesKey,
    trigger: &str,
) -> Result<Vec<BackupRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE kind = ? AND project_id IS ? AND database_id IS ? AND note IS ?
         AND trigger = ? AND status = 'success' ORDER BY created_at DESC, id DESC"
    )))
    .bind(&key.kind)
    .bind(&key.project_id)
    .bind(&key.database_id)
    .bind(&key.note)
    .bind(trigger)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

/// Every series that has successful rows made by `trigger`.
pub async fn series_keys(
    conn: &mut SqliteConnection,
    trigger: &str,
) -> Result<Vec<SeriesKey>, CoreError> {
    sqlx::query_as(
        "SELECT DISTINCT kind, project_id, database_id, note FROM backups
         WHERE trigger = ? AND status = 'success'",
    )
    .bind(trigger)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

/// Failed rows older than `cutoff_ms`.
pub async fn old_failed(
    conn: &mut SqliteConnection,
    cutoff_ms: i64,
) -> Result<Vec<BackupRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE status = 'failed' AND created_at < ?"
    )))
    .bind(cutoff_ms)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn record_download(
    conn: &mut SqliteConnection,
    id: &str,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE backups SET download_count = download_count + 1, downloaded_at = ? WHERE id = ?",
    )
    .bind(now)
    .bind(id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn delete(conn: &mut SqliteConnection, id: &str) -> Result<bool, CoreError> {
    sqlx::query("DELETE FROM backups WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected() > 0)
        .map_err(db_err)
}
