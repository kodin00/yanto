//! `audit_logs` table: append-only.

use sqlx::{FromRow, SqliteConnection};

use super::db_err;
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct AuditRow {
    pub id: String,
    pub actor_type: String,
    pub actor_id: Option<String>,
    pub actor_label: String,
    pub action: String,
    pub entity_type: String,
    pub entity_id: Option<String>,
    pub project_id: Option<String>,
    pub ip: Option<String>,
    pub channel: String,
    pub outcome: String,
    pub request_id: Option<String>,
    pub metadata: String,
    pub created_at: i64,
}

pub async fn insert(conn: &mut SqliteConnection, r: &AuditRow) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO audit_logs (id, actor_type, actor_id, actor_label, action, entity_type, entity_id,
            project_id, ip, channel, outcome, request_id, metadata, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&r.id)
    .bind(&r.actor_type)
    .bind(&r.actor_id)
    .bind(&r.actor_label)
    .bind(&r.action)
    .bind(&r.entity_type)
    .bind(&r.entity_id)
    .bind(&r.project_id)
    .bind(&r.ip)
    .bind(&r.channel)
    .bind(&r.outcome)
    .bind(&r.request_id)
    .bind(&r.metadata)
    .bind(r.created_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

/// Which rows a reader may see.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// `None` = every row; `Some(list)` = only rows whose project is in the list.
    pub visible_projects: Option<Vec<String>>,
    /// Narrow to one Project.
    pub project_id: Option<String>,
    /// Keyset cursor: rows strictly older than `(created_at, id)`.
    pub before: Option<(i64, String)>,
    pub limit: i64,
}

/// Newest first. Project visibility is applied in SQL so a Member never loads rows they may not see.
pub async fn list(
    conn: &mut SqliteConnection,
    f: &AuditFilter,
) -> Result<Vec<AuditRow>, CoreError> {
    let visible = f
        .visible_projects
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()));
    sqlx::query_as(
        "SELECT id, actor_type, actor_id, actor_label, action, entity_type, entity_id, project_id,
                ip, channel, outcome, request_id, metadata, created_at
         FROM audit_logs
         WHERE (?1 IS NULL OR project_id = ?1)
           AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND id < ?3))
           AND (?4 IS NULL OR project_id IN (SELECT value FROM json_each(?4)))
         ORDER BY created_at DESC, id DESC
         LIMIT ?5",
    )
    .bind(&f.project_id)
    .bind(f.before.as_ref().map(|b| b.0))
    .bind(f.before.as_ref().map(|b| b.1.clone()))
    .bind(visible)
    .bind(f.limit)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}
