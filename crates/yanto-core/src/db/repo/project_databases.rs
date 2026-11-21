//! `project_databases` table (declared Postgres/MySQL services, 02 4.2).

use sqlx::{FromRow, SqliteConnection};

use super::db_err;
use crate::error::CoreError;

#[derive(Debug, Clone, FromRow)]
pub struct DatabaseRow {
    pub id: String,
    pub project_id: String,
    pub engine: String,
    pub compose_service: String,
    pub database: Option<String>,
    pub user_env: Option<String>,
    pub password_env: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

const COLS: &str = "id, project_id, engine, compose_service, \"database\", user_env, password_env, created_at, updated_at";

pub async fn list(
    conn: &mut SqliteConnection,
    project_id: &str,
) -> Result<Vec<DatabaseRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM project_databases WHERE project_id = ? ORDER BY compose_service"
    )))
    .bind(project_id)
    .fetch_all(conn)
    .await
    .map_err(db_err)
}

pub async fn get(conn: &mut SqliteConnection, id: &str) -> Result<Option<DatabaseRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM project_databases WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

pub async fn by_service(
    conn: &mut SqliteConnection,
    project_id: &str,
    service: &str,
) -> Result<Option<DatabaseRow>, CoreError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM project_databases WHERE project_id = ? AND compose_service = ?"
    )))
    .bind(project_id)
    .bind(service)
    .fetch_optional(conn)
    .await
    .map_err(db_err)
}

/// Insert or update by `(project_id, compose_service)` so an existing row keeps its id (Backups
/// reference it).
pub async fn upsert(conn: &mut SqliteConnection, d: &DatabaseRow) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO project_databases (id, project_id, engine, compose_service, \"database\", user_env,
            password_env, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (project_id, compose_service) DO UPDATE SET engine = excluded.engine,
            \"database\" = excluded.\"database\", user_env = excluded.user_env,
            password_env = excluded.password_env, updated_at = excluded.updated_at",
    )
    .bind(&d.id)
    .bind(&d.project_id)
    .bind(&d.engine)
    .bind(&d.compose_service)
    .bind(&d.database)
    .bind(&d.user_env)
    .bind(&d.password_env)
    .bind(d.created_at)
    .bind(d.updated_at)
    .execute(conn)
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn delete(conn: &mut SqliteConnection, id: &str) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM project_databases WHERE id = ?")
        .bind(id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(db_err)
}
