//! `projects` and `project_ports` tables.

use sqlx::{FromRow, SqliteConnection};

use super::{db_err, is_unique_violation};
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub source_kind: String,
    pub git_url: Option<String>,
    pub branch: Option<String>,
    pub image_ref: Option<String>,
    pub compose_path: String,
    pub compose_content: Option<String>,
    pub env_file: String,
    pub auto_start: bool,
    pub manual_deploy_enabled: bool,
    pub github_webhook_enabled: bool,
    pub deploy_token_hash: String,
    pub deploy_token_enc: String,
    pub deploy_key_id: Option<String>,
    pub registry_credential_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ProjectRow {
    pub fn is_git(&self) -> bool {
        self.source_kind == "git"
    }
}

const COLS: &str = "id, name, slug, source_kind, git_url, branch, image_ref, compose_path,
    compose_content, env_file, auto_start, manual_deploy_enabled, github_webhook_enabled,
    deploy_token_hash, deploy_token_enc, deploy_key_id, registry_credential_id, created_at, updated_at";

pub async fn insert(conn: &mut SqliteConnection, p: &ProjectRow) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO projects (id, name, slug, source_kind, git_url, branch, image_ref, compose_path,
            compose_content, env_file, auto_start, manual_deploy_enabled, github_webhook_enabled,
            deploy_token_hash, deploy_token_enc, deploy_key_id, registry_credential_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&p.id)
    .bind(&p.name)
    .bind(&p.slug)
    .bind(&p.source_kind)
    .bind(&p.git_url)
    .bind(&p.branch)
    .bind(&p.image_ref)
    .bind(&p.compose_path)
    .bind(&p.compose_content)
    .bind(&p.env_file)
    .bind(p.auto_start)
    .bind(p.manual_deploy_enabled)
    .bind(p.github_webhook_enabled)
    .bind(&p.deploy_token_hash)
    .bind(&p.deploy_token_enc)
    .bind(&p.deploy_key_id)
    .bind(&p.registry_credential_id)
    .bind(p.created_at)
    .bind(p.updated_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("a project with this slug already exists")
        } else {
            db_err(e)
        }
    })
}

/// Persist every mutable column of `p` (identity, slug and source kind never change).
pub async fn save(conn: &mut SqliteConnection, p: &ProjectRow) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE projects SET name = ?, git_url = ?, branch = ?, image_ref = ?, compose_path = ?,
            compose_content = ?, env_file = ?, auto_start = ?, manual_deploy_enabled = ?,
            github_webhook_enabled = ?, deploy_token_hash = ?, deploy_token_enc = ?,
            deploy_key_id = ?, registry_credential_id = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&p.name)
    .bind(&p.git_url)
    .bind(&p.branch)
    .bind(&p.image_ref)
    .bind(&p.compose_path)
    .bind(&p.compose_content)
    .bind(&p.env_file)
    .bind(p.auto_start)
    .bind(p.manual_deploy_enabled)
    .bind(p.github_webhook_enabled)
    .bind(&p.deploy_token_hash)
    .bind(&p.deploy_token_enc)
    .bind(&p.deploy_key_id)
    .bind(&p.registry_credential_id)
    .bind(p.updated_at)
    .bind(&p.id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn get(conn: &mut SqliteConnection, id: &str) -> Result<Option<ProjectRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM projects WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

pub async fn list(conn: &mut SqliteConnection) -> Result<Vec<ProjectRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM projects ORDER BY created_at DESC, id"
    )))
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn delete(conn: &mut SqliteConnection, id: &str) -> Result<(), CoreError> {
    // M7: a Hostname publishes the Project on a provider; deleting the row would leave that
    // public route behind (and its port free to be reused by another Project).
    let hostnames: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hostnames WHERE project_id = ?")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;
    if hostnames > 0 {
        return Err(CoreError::conflict(format!(
            "{hostnames} Hostname(s) still publish this project; delete them first"
        )));
    }
    sqlx::query("DELETE FROM projects WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}

/// How many of `ids` exist (used to reject grants for unknown Projects).
pub async fn count_existing(conn: &mut SqliteConnection, ids: &[&str]) -> Result<usize, CoreError> {
    let mut n = 0;
    for id in ids {
        let found: Option<String> = sqlx::query_scalar("SELECT id FROM projects WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db_err)?;
        n += usize::from(found.is_some());
    }
    Ok(n)
}

pub async fn count_for_deploy_key(
    conn: &mut SqliteConnection,
    key_id: &str,
) -> Result<Vec<String>, CoreError> {
    sqlx::query_scalar("SELECT name FROM projects WHERE deploy_key_id = ? ORDER BY name")
        .bind(key_id)
        .fetch_all(conn)
        .await
        .map_err(db_err)
}

pub async fn names_for_registry_credential(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Vec<String>, CoreError> {
    sqlx::query_scalar("SELECT name FROM projects WHERE registry_credential_id = ? ORDER BY name")
        .bind(id)
        .fetch_all(conn)
        .await
        .map_err(db_err)
}

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct PortRow {
    pub id: String,
    pub project_id: String,
    pub label: String,
    pub compose_service: Option<String>,
    pub host_port: i64,
    pub container_port: i64,
    pub bind_address: String,
    pub protocol: String,
    pub created_at: i64,
}

const PORT_COLS: &str = "id, project_id, label, compose_service, host_port, container_port, bind_address, protocol, created_at";

pub async fn ports_for(
    conn: &mut SqliteConnection,
    project_id: &str,
) -> Result<Vec<PortRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {PORT_COLS} FROM project_ports WHERE project_id = ? ORDER BY host_port, protocol"
    )))
    .bind(project_id)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

/// Declared ports of every Project other than `except` (collision check, rule b).
pub async fn ports_of_others(
    conn: &mut SqliteConnection,
    except: &str,
) -> Result<Vec<(String, PortRow)>, CoreError> {
    let rows: Vec<PortRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {PORT_COLS} FROM project_ports WHERE project_id <> ?"
    )))
    .bind(except)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let name: String = sqlx::query_scalar("SELECT name FROM projects WHERE id = ?")
            .bind(&r.project_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(db_err)?;
        out.push((name, r));
    }
    Ok(out)
}

pub async fn insert_port(conn: &mut SqliteConnection, p: &PortRow) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO project_ports (id, project_id, label, compose_service, host_port, container_port,
            bind_address, protocol, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&p.id)
    .bind(&p.project_id)
    .bind(&p.label)
    .bind(&p.compose_service)
    .bind(p.host_port)
    .bind(p.container_port)
    .bind(&p.bind_address)
    .bind(&p.protocol)
    .bind(p.created_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("host port or label is already declared")
        } else {
            db_err(e)
        }
    })
}

pub async fn delete_port(conn: &mut SqliteConnection, id: &str) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM project_ports WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(|e| {
            if e.as_database_error()
                .is_some_and(|d| d.is_foreign_key_violation())
            {
                CoreError::conflict("a Hostname uses this port; delete the Hostname first")
            } else {
                db_err(e)
            }
        })
}

pub async fn update_port(
    conn: &mut SqliteConnection,
    id: &str,
    label: &str,
    compose_service: Option<&str>,
    container_port: i64,
) -> Result<(), CoreError> {
    sqlx::query(
        "UPDATE project_ports SET label = ?, compose_service = ?, container_port = ? WHERE id = ?",
    )
    .bind(label)
    .bind(compose_service)
    .bind(container_port)
    .bind(id)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(|e| {
        if is_unique_violation(&e) {
            CoreError::conflict("port label is already used in this project")
        } else {
            db_err(e)
        }
    })
}
