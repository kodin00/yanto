//! `deployments` (also the queue) and `deployment_logs` tables.

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection};

use super::{db_err, is_unique_violation};
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct DeploymentRow {
    pub id: String,
    pub project_id: String,
    pub status: String,
    pub trigger: String,
    pub triggered_by: Option<String>,
    pub target_ref: Option<String>,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    pub rollback_from_deployment_id: Option<String>,
    pub image_digest: Option<String>,
    pub compose_snapshot: Option<String>,
    pub env_snapshot_enc: Option<String>,
    pub pending_env_enc: Option<String>,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    pub log_bytes: i64,
    pub log_truncated: bool,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

impl DeploymentRow {
    pub fn is_terminal(&self) -> bool {
        !matches!(self.status.as_str(), "queued" | "running")
    }
}

const COLS: &str = r#"id, project_id, status, "trigger" AS trigger, triggered_by, target_ref, commit_sha,
    commit_message, rollback_from_deployment_id, image_digest, compose_snapshot, env_snapshot_enc,
    pending_env_enc, exit_code, error, log_bytes, log_truncated, queued_at, started_at, finished_at"#;

pub async fn insert(conn: &mut SqliteConnection, d: &DeploymentRow) -> Result<(), CoreError> {
    sqlx::query(
        r#"INSERT INTO deployments (id, project_id, status, "trigger", triggered_by, target_ref,
            rollback_from_deployment_id, compose_snapshot, env_snapshot_enc, pending_env_enc, queued_at)
           VALUES (?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&d.id)
    .bind(&d.project_id)
    .bind(&d.trigger)
    .bind(&d.triggered_by)
    .bind(&d.target_ref)
    .bind(&d.rollback_from_deployment_id)
    .bind(&d.compose_snapshot)
    .bind(&d.env_snapshot_enc)
    .bind(&d.pending_env_enc)
    .bind(d.queued_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("a deployment is already queued for this project")
        } else {
            db_err(e)
        }
    })
}

pub async fn get(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<DeploymentRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM deployments WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

pub async fn queued_for(
    conn: &mut SqliteConnection,
    project_id: &str,
) -> Result<Option<DeploymentRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM deployments WHERE project_id = ? AND status = 'queued'"
    )))
    .bind(project_id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

pub async fn running_for(
    conn: &mut SqliteConnection,
    project_id: &str,
) -> Result<Option<DeploymentRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM deployments WHERE project_id = ? AND status = 'running'"
    )))
    .bind(project_id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

#[derive(Debug, Clone, Default)]
pub struct DeploymentFilter {
    /// `None` = every Project (Owner); `Some(ids)` = only these.
    pub visible_projects: Option<Vec<String>>,
    pub project_id: Option<String>,
    pub status: Option<String>,
    pub limit: i64,
}

pub async fn list(
    conn: &mut SqliteConnection,
    f: &DeploymentFilter,
) -> Result<Vec<DeploymentRow>, CoreError> {
    let mut sql = format!("SELECT {COLS} FROM deployments WHERE 1 = 1");
    if let Some(v) = &f.visible_projects {
        if v.is_empty() {
            return Ok(Vec::new());
        }
        sql.push_str(" AND project_id IN (");
        sql.push_str(&vec!["?"; v.len()].join(","));
        sql.push(')');
    }
    if f.project_id.is_some() {
        sql.push_str(" AND project_id = ?");
    }
    if f.status.is_some() {
        sql.push_str(" AND status = ?");
    }
    sql.push_str(" ORDER BY queued_at DESC, rowid DESC LIMIT ?");
    let mut q = sqlx::query_as::<_, DeploymentRow>(sqlx::AssertSqlSafe(sql));
    if let Some(v) = &f.visible_projects {
        for p in v {
            q = q.bind(p);
        }
    }
    if let Some(p) = &f.project_id {
        q = q.bind(p);
    }
    if let Some(s) = &f.status {
        q = q.bind(s);
    }
    q.bind(f.limit).fetch_all(conn).await.map_err(db_err)
}

/// Claim the oldest queued Deployment whose Project has nothing running. The
/// partial unique index on `status = 'running'` is the final guard.
pub async fn claim_next(
    conn: &mut SqliteConnection,
    now: i64,
) -> Result<Option<DeploymentRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE deployments SET status = 'running', started_at = ?
         WHERE id = (
            SELECT d.id FROM deployments d
            WHERE d.status = 'queued'
              AND NOT EXISTS (SELECT 1 FROM deployments r WHERE r.project_id = d.project_id AND r.status = 'running')
            ORDER BY d.queued_at, d.rowid LIMIT 1)
         RETURNING {COLS}"
    )))
    .bind(now)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

pub async fn set_commit(
    conn: &mut SqliteConnection,
    id: &str,
    sha: Option<&str>,
    message: Option<&str>,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE deployments SET commit_sha = ?, commit_message = ? WHERE id = ?")
        .bind(sha)
        .bind(message)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn set_snapshots(
    conn: &mut SqliteConnection,
    id: &str,
    compose: Option<&str>,
    env_enc: Option<&str>,
) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE deployments SET compose_snapshot = ?, env_snapshot_enc = ?, pending_env_enc = NULL WHERE id = ?",
    )
    .bind(compose)
    .bind(env_enc)
    .bind(id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn set_image_digest(
    conn: &mut SqliteConnection,
    id: &str,
    digest: &str,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE deployments SET image_digest = ? WHERE id = ?")
        .bind(digest)
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

/// Move a Deployment to a terminal state. Only a `queued` or `running` row changes, so a late
/// finish cannot overwrite a cancel. Returns whether it changed. Cancelling a queued row goes
/// through [`cancel_queued`] instead.
#[allow(clippy::too_many_arguments)]
pub async fn finish(
    conn: &mut SqliteConnection,
    id: &str,
    status: &str,
    exit_code: Option<i64>,
    error: Option<&str>,
    now: i64,
) -> Result<bool, CoreError> {
    let r = sqlx::query(
        "UPDATE deployments SET status = ?, exit_code = ?, error = ?, finished_at = ?, pending_env_enc = NULL
         WHERE id = ? AND status IN ('queued', 'running')",
    )
    .bind(status)
    .bind(exit_code)
    .bind(error)
    .bind(now)
    .bind(id)
    .execute(conn)
    .await
    .map_err(db_err)?;
    Ok(r.rows_affected() == 1)
}

/// Cancel a Deployment that is still `queued`. `false` when the worker claimed it first: a
/// `running` row must be cancelled through its token, never just relabelled (the process would
/// keep running and the Project's running slot would be freed under it).
pub async fn cancel_queued(
    conn: &mut SqliteConnection,
    id: &str,
    now: i64,
) -> Result<bool, CoreError> {
    let r = sqlx::query(
        "UPDATE deployments SET status = 'cancelled', exit_code = 130, error = NULL, finished_at = ?,
         pending_env_enc = NULL
         WHERE id = ? AND status = 'queued'",
    )
    .bind(now)
    .bind(id)
    .execute(conn)
    .await
    .map_err(db_err)?;
    Ok(r.rows_affected() == 1)
}

/// Startup recovery: every `running` row becomes `interrupted`. Returns their ids.
pub async fn interrupt_running(
    conn: &mut SqliteConnection,
    now: i64,
) -> Result<Vec<String>, CoreError> {
    sqlx::query_scalar(
        "UPDATE deployments SET status = 'interrupted', finished_at = ?, pending_env_enc = NULL,
            error = 'Yanto restarted during this Deployment'
         WHERE status = 'running' RETURNING id",
    )
    .bind(now)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

/// Successful Deployments of a Project, newest first (rollback targets).
pub async fn successes(
    conn: &mut SqliteConnection,
    project_id: &str,
    limit: i64,
) -> Result<Vec<DeploymentRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM deployments WHERE project_id = ? AND status = 'success'
         ORDER BY queued_at DESC, rowid DESC LIMIT ?"
    )))
    .bind(project_id)
    .bind(limit)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

/// Keep the newest `keep` finished Deployments per Project; returns rows removed.
pub async fn prune(
    conn: &mut SqliteConnection,
    project_id: &str,
    keep: i64,
) -> Result<u64, CoreError> {
    let r = sqlx::query(
        "DELETE FROM deployments WHERE project_id = ? AND status NOT IN ('queued', 'running')
           AND id NOT IN (
             SELECT id FROM deployments WHERE project_id = ? AND status NOT IN ('queued', 'running')
             ORDER BY queued_at DESC, rowid DESC LIMIT ?)",
    )
    .bind(project_id)
    .bind(project_id)
    .bind(keep)
    .execute(conn)
    .await
    .map_err(db_err)?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct LogChunk {
    pub seq: i64,
    pub ts: i64,
    pub stream: String,
    pub data: String,
}

pub async fn max_seq(conn: &mut SqliteConnection, id: &str) -> Result<i64, CoreError> {
    let v: Option<i64> =
        sqlx::query_scalar("SELECT MAX(seq) FROM deployment_logs WHERE deployment_id = ?")
            .bind(id)
            .fetch_one(conn)
            .await
            .map_err(db_err)?;
    Ok(v.unwrap_or(0))
}

pub async fn append_log(
    conn: &mut SqliteConnection,
    id: &str,
    chunk: &LogChunk,
) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO deployment_logs (deployment_id, seq, ts, stream, data) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(chunk.seq)
    .bind(chunk.ts)
    .bind(&chunk.stream)
    .bind(&chunk.data)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn logs_after(
    conn: &mut SqliteConnection,
    id: &str,
    after_seq: i64,
    limit: i64,
) -> Result<Vec<LogChunk>, CoreError> {
    sqlx::query_as(
        "SELECT seq, ts, stream, data FROM deployment_logs
         WHERE deployment_id = ? AND seq > ? ORDER BY seq LIMIT ?",
    )
    .bind(id)
    .bind(after_seq)
    .bind(limit)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

/// Add to the running total of stored log bytes.
pub async fn add_log_bytes(
    conn: &mut SqliteConnection,
    id: &str,
    delta: i64,
) -> Result<i64, CoreError> {
    sqlx::query_scalar(
        "UPDATE deployments SET log_bytes = log_bytes + ? WHERE id = ? RETURNING log_bytes",
    )
    .bind(delta)
    .bind(id)
    .fetch_one(conn)
    .await
    .map_err(db_err)
}

pub const TRUNCATION_MARKER: &str = "[earlier log output was truncated]\n";

/// Drop the oldest chunks until at most `max_bytes` remain; keeps a truncation marker
/// in place of the dropped head and flags the Deployment.
pub async fn truncate_logs(
    conn: &mut SqliteConnection,
    id: &str,
    max_bytes: i64,
    now: i64,
) -> Result<(), CoreError> {
    let chunks: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT seq, length(CAST(data AS BLOB)) FROM deployment_logs
         WHERE deployment_id = ? AND stream <> 'system' ORDER BY seq DESC",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let mut kept = 0i64;
    let mut cutoff: Option<i64> = None;
    for (seq, len) in &chunks {
        if kept + len > max_bytes {
            cutoff = Some(*seq);
            break;
        }
        kept += len;
    }
    let Some(cutoff) = cutoff else {
        return Ok(());
    };
    sqlx::query(
        "DELETE FROM deployment_logs WHERE deployment_id = ? AND stream <> 'system' AND seq <= ?",
    )
    .bind(id)
    .bind(cutoff)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    // One marker at the current cut: drop the ones earlier truncations left behind.
    sqlx::query(
        "DELETE FROM deployment_logs WHERE deployment_id = ? AND stream = 'system' AND data = ?",
    )
    .bind(id)
    .bind(TRUNCATION_MARKER)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    let marker = TRUNCATION_MARKER;
    sqlx::query(
        "INSERT OR REPLACE INTO deployment_logs (deployment_id, seq, ts, stream, data) VALUES (?, ?, ?, 'system', ?)",
    )
    .bind(id)
    .bind(cutoff)
    .bind(now)
    .bind(marker)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    // Recount everything stored (system lines included) so the cap stays honest.
    sqlx::query(
        "UPDATE deployments SET log_truncated = 1, log_bytes = (
             SELECT COALESCE(SUM(length(CAST(data AS BLOB))), 0) FROM deployment_logs
             WHERE deployment_id = ?1)
         WHERE id = ?1",
    )
    .bind(id)
    .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}
