//! `cron_entries` and `cron_runs`.

use sqlx::{FromRow, SqliteConnection};

use super::{db_err, is_unique_violation};
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct CronEntryRow {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub run_as_user: String,
    pub command: String,
    pub overlap: String,
    pub timeout_s: Option<i64>,
    pub enabled: bool,
    pub project_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct CronRunRow {
    pub id: String,
    pub entry_id: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub output_tail: Option<String>,
}

const ENTRY: &str = "SELECT id, name, schedule, run_as_user, command, overlap, timeout_s, enabled,
    project_id, created_at, updated_at FROM cron_entries";

fn dup(e: sqlx::Error) -> CoreError {
    if is_unique_violation(&e) {
        CoreError::conflict("another Cron Entry already has that name")
    } else {
        db_err(e)
    }
}

pub struct EntryFields<'a> {
    pub name: &'a str,
    pub schedule: &'a str,
    pub run_as_user: &'a str,
    pub command: &'a str,
    pub overlap: &'a str,
    pub timeout_s: Option<i64>,
    pub enabled: bool,
}

pub async fn insert(
    conn: &mut SqliteConnection,
    id: &str,
    f: &EntryFields<'_>,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO cron_entries (id, name, schedule, run_as_user, command, overlap, timeout_s,
            enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(f.name)
    .bind(f.schedule)
    .bind(f.run_as_user)
    .bind(f.command)
    .bind(f.overlap)
    .bind(f.timeout_s)
    .bind(f.enabled)
    .bind(now)
    .bind(now)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(dup)
}

pub async fn update(
    conn: &mut SqliteConnection,
    id: &str,
    f: &EntryFields<'_>,
    now: i64,
) -> Result<bool, CoreError> {
    sqlx::query(
        "UPDATE cron_entries SET name = ?, schedule = ?, run_as_user = ?, command = ?, overlap = ?,
            timeout_s = ?, enabled = ?, updated_at = ? WHERE id = ?",
    )
    .bind(f.name)
    .bind(f.schedule)
    .bind(f.run_as_user)
    .bind(f.command)
    .bind(f.overlap)
    .bind(f.timeout_s)
    .bind(f.enabled)
    .bind(now)
    .bind(id)
    .execute(conn)
    .await
    .map(|r| r.rows_affected() > 0)
    .map_err(dup)
}

pub async fn set_enabled(
    conn: &mut SqliteConnection,
    id: &str,
    enabled: bool,
    now: i64,
) -> Result<bool, CoreError> {
    sqlx::query("UPDATE cron_entries SET enabled = ?, updated_at = ? WHERE id = ?")
        .bind(enabled)
        .bind(now)
        .bind(id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected() > 0)
        .map_err(db_err)
}

pub async fn delete(conn: &mut SqliteConnection, id: &str) -> Result<bool, CoreError> {
    sqlx::query("DELETE FROM cron_entries WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|r| r.rows_affected() > 0)
        .map_err(db_err)
}

pub async fn get(conn: &mut SqliteConnection, id: &str) -> Result<Option<CronEntryRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY} WHERE id = ?")))
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db_err)
}

pub async fn list(conn: &mut SqliteConnection) -> Result<Vec<CronEntryRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ENTRY} ORDER BY name COLLATE NOCASE, id"
    )))
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn run_insert(
    conn: &mut SqliteConnection,
    id: &str,
    entry_id: &str,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO cron_runs (id, entry_id, status, started_at) VALUES (?, ?, 'running', ?)",
    )
    .bind(id)
    .bind(entry_id)
    .bind(now)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn run_finish(
    conn: &mut SqliteConnection,
    id: &str,
    status: &str,
    exit_code: Option<i32>,
    finished_at: i64,
    output_tail: Option<&str>,
) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE cron_runs SET status = ?, exit_code = ?, finished_at = ?, output_tail = ? WHERE id = ?",
    )
    .bind(status)
    .bind(exit_code.map(i64::from))
    .bind(finished_at)
    .bind(output_tail)
    .bind(id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn runs_list(
    conn: &mut SqliteConnection,
    entry_id: &str,
    limit: i64,
) -> Result<Vec<CronRunRow>, CoreError> {
    sqlx::query_as(
        "SELECT id, entry_id, status, exit_code, started_at, finished_at, output_tail
         FROM cron_runs WHERE entry_id = ? ORDER BY started_at DESC, id DESC LIMIT ?",
    )
    .bind(entry_id)
    .bind(limit)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

/// Keep the newest `keep` runs and anything newer than `cutoff_ms` (5.3: last 50 and 30 days).
pub async fn runs_prune(
    conn: &mut SqliteConnection,
    entry_id: &str,
    keep: i64,
    cutoff_ms: i64,
) -> Result<u64, CoreError> {
    sqlx::query(
        "DELETE FROM cron_runs WHERE entry_id = ?1 AND status != 'running'
           AND (started_at < ?2 OR id NOT IN (
               SELECT id FROM cron_runs WHERE entry_id = ?1
               ORDER BY started_at DESC, id DESC LIMIT ?3))",
    )
    .bind(entry_id)
    .bind(cutoff_ms)
    .bind(keep)
    .execute(conn)
    .await
    .map(|r| r.rows_affected())
    .map_err(db_err)
}
