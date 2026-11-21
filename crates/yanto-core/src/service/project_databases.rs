//! Declared Postgres/MySQL services of a Project (`project_databases_list|set`, 03 a.2, 04 7.2).
//! Backups dump these through `docker exec`; no credential is stored, only the *names* of the
//! environment variables that hold them inside the container.

use serde::Serialize;
use serde_json::json;

use super::audit::{self, AuditActor, AuditEvent};
use super::projects::{ProjectService, not_found};
use crate::actor::{Access, Actor, Need, OpSpec, Permission, Scope};
use crate::crypto::new_id;
use crate::db::repo::project_databases::{self as repo, DatabaseRow};
use crate::db::repo::{db_err, projects};
use crate::error::CoreError;

pub const OP_PROJECT_DATABASES_LIST: OpSpec = OpSpec::new(
    "project_databases_list",
    Access::Project(Need::View),
    Scope::Read,
);
pub const OP_PROJECT_DATABASES_SET: OpSpec = OpSpec::new(
    "project_databases_set",
    Access::Project(Need::Permission(Permission::Config)),
    Scope::Write,
);

pub const OPERATIONS: &[&OpSpec] = &[&OP_PROJECT_DATABASES_LIST, &OP_PROJECT_DATABASES_SET];

const MAX_DECLARED: usize = 20;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DatabaseDto {
    pub id: String,
    pub engine: String,
    pub compose_service: String,
    pub database: Option<String>,
    pub user_env: Option<String>,
    pub password_env: Option<String>,
}

impl From<DatabaseRow> for DatabaseDto {
    fn from(r: DatabaseRow) -> Self {
        Self {
            id: r.id,
            engine: r.engine,
            compose_service: r.compose_service,
            database: r.database,
            user_env: r.user_env,
            password_env: r.password_env,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct DatabaseInput {
    pub engine: String,
    pub compose_service: String,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub user_env: Option<String>,
    #[serde(default)]
    pub password_env: Option<String>,
}

/// Compose service and database names end up in `docker exec` arguments: `^[A-Za-z0-9._-]+$`.
fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Environment variable names are interpolated into a shell string: `^[A-Za-z_][A-Za-z0-9_]*$`.
fn env_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && !s.as_bytes()[0].is_ascii_digit()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn blank(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

impl ProjectService {
    pub async fn databases_list(
        &self,
        actor: &Actor,
        id: &str,
    ) -> Result<Vec<DatabaseDto>, CoreError> {
        actor.authorize(&OP_PROJECT_DATABASES_LIST, Some(id))?;
        let mut conn = self.ctx().db.reader().acquire().await.map_err(db_err)?;
        projects::get(&mut conn, id).await?.ok_or_else(not_found)?;
        Ok(repo::list(&mut conn, id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Replace the declarations. Rows are matched by service so ids (and Backups pointing at
    /// them) survive an edit; services that are no longer listed are deleted.
    pub async fn databases_set(
        &self,
        actor: &Actor,
        id: &str,
        inputs: &[DatabaseInput],
    ) -> Result<Vec<DatabaseDto>, CoreError> {
        actor.authorize(&OP_PROJECT_DATABASES_SET, Some(id))?;
        if inputs.len() > MAX_DECLARED {
            return Err(CoreError::validation(
                "databases",
                "at most 20 databases per project",
            ));
        }
        let mut wanted: Vec<DatabaseInput> = Vec::with_capacity(inputs.len());
        for i in inputs {
            let engine = i.engine.trim();
            if !matches!(engine, "postgres" | "mysql") {
                return Err(CoreError::validation(
                    "engine",
                    "engine must be postgres or mysql",
                ));
            }
            let service = i.compose_service.trim();
            if !name_ok(service) {
                return Err(CoreError::validation(
                    "compose_service",
                    "service names use letters, digits, '.', '_' and '-'",
                ));
            }
            let database = blank(&i.database);
            if database.as_deref().is_some_and(|d| !name_ok(d)) {
                return Err(CoreError::validation(
                    "database",
                    "database names use letters, digits, '.', '_' and '-'",
                ));
            }
            let (user_env, password_env) = (blank(&i.user_env), blank(&i.password_env));
            for (field, v) in [("user_env", &user_env), ("password_env", &password_env)] {
                if v.as_deref().is_some_and(|v| !env_name_ok(v)) {
                    return Err(CoreError::validation(
                        field,
                        "give the NAME of an environment variable in the container, not a value",
                    ));
                }
            }
            if wanted.iter().any(|w| w.compose_service == service) {
                return Err(CoreError::validation(
                    "compose_service",
                    "each service can be declared once",
                ));
            }
            wanted.push(DatabaseInput {
                engine: engine.to_owned(),
                compose_service: service.to_owned(),
                database,
                user_env,
                password_env,
            });
        }
        let now = self.ctx().now();
        let mut tx = self.ctx().db.begin_write().await?;
        projects::get(&mut tx, id).await?.ok_or_else(not_found)?;
        let existing = repo::list(&mut tx, id).await?;
        for old in &existing {
            if !wanted.iter().any(|w| w.compose_service == old.compose_service) {
                repo::delete(&mut tx, &old.id).await?;
            }
        }
        for w in &wanted {
            let prev = existing
                .iter()
                .find(|o| o.compose_service == w.compose_service);
            repo::upsert(
                &mut tx,
                &DatabaseRow {
                    id: prev.map_or_else(|| new_id("pdb"), |p| p.id.clone()),
                    project_id: id.to_owned(),
                    engine: w.engine.clone(),
                    compose_service: w.compose_service.clone(),
                    database: w.database.clone(),
                    user_env: w.user_env.clone(),
                    password_env: w.password_env.clone(),
                    created_at: prev.map_or(now, |p| p.created_at),
                    updated_at: now,
                },
            )
            .await?;
        }
        audit::write(
            self.ctx(),
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("project.databases_set", "project")
                .entity(id)
                .project(id)
                .meta(json!({ "services": wanted.iter().map(|w| w.compose_service.as_str()).collect::<Vec<_>>() })),
        )
        .await?;
        let out = repo::list(&mut tx, id).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(out.into_iter().map(Into::into).collect())
    }
}
