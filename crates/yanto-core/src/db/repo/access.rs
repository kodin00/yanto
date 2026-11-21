//! `user_project_access` and `user_project_permissions`.

use std::collections::{BTreeMap, BTreeSet};

use sqlx::SqliteConnection;

use super::db_err;
use crate::actor::Permission;
use crate::error::CoreError;

pub type Grants = BTreeMap<String, BTreeSet<Permission>>;

pub async fn load(conn: &mut SqliteConnection, user_id: &str) -> Result<Grants, CoreError> {
    let access: Vec<String> =
        sqlx::query_scalar("SELECT project_id FROM user_project_access WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(&mut *conn)
            .await
            .map_err(db_err)?;
    let mut grants: Grants = access.into_iter().map(|p| (p, BTreeSet::new())).collect();
    let perms: Vec<(String, String)> = sqlx::query_as(
        "SELECT project_id, permission FROM user_project_permissions WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    for (project, perm) in perms {
        if let (Some(set), Ok(p)) = (grants.get_mut(&project), perm.parse::<Permission>()) {
            set.insert(p);
        }
    }
    Ok(grants)
}

/// Replace the user's whole grant set (access rows and their permissions).
pub async fn replace(
    conn: &mut SqliteConnection,
    user_id: &str,
    grants: &Grants,
    now: i64,
) -> Result<(), CoreError> {
    sqlx::query("DELETE FROM user_project_access WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    for (project, perms) in grants {
        sqlx::query(
            "INSERT INTO user_project_access (user_id, project_id, created_at, updated_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(project)
        .bind(now)
        .bind(now)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
        for p in perms {
            sqlx::query(
                "INSERT INTO user_project_permissions (user_id, project_id, permission)
                 VALUES (?, ?, ?)",
            )
            .bind(user_id)
            .bind(project)
            .bind(p.as_str())
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }
    }
    Ok(())
}
