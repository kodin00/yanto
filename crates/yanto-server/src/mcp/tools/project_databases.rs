//! Declared Postgres/MySQL services of a Project (only names are stored, never credentials).

use schemars::JsonSchema;
use serde::Deserialize;
use yanto_core::actor::Scope;
use yanto_core::service::project_databases::DatabaseInput;

use crate::mcp::{Class, ToolDef, ToolSet, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_project_databases_list",
        "project_databases_list",
        "List the databases declared for a Project.",
        Scope::Read,
        Class::Read,
        |s, a, args: IdArgs| async move { to_json(&s.projects.databases_list(&a, &args.id).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_project_databases_set",
        "project_databases_set",
        "Replace the databases declared for a Project (engine postgres or mysql, plus the Compose service). Services not listed are removed.",
        Scope::Write,
        Class::Write,
        |s, a, args: SetArgs| async move {
            let inputs: Vec<DatabaseInput> = args
                .databases
                .into_iter()
                .map(|d| DatabaseInput {
                    engine: d.engine,
                    compose_service: d.compose_service,
                    database: d.database,
                    user_env: d.user_env,
                    password_env: d.password_env,
                })
                .collect();
            to_json(&s.projects.databases_set(&a, &args.id, &inputs).await?)
        },
    ));
}

#[derive(Debug, Deserialize, JsonSchema)]
struct IdArgs {
    /// Project id.
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DatabaseArgs {
    /// `postgres` or `mysql`.
    engine: String,
    /// Compose service that runs the database.
    compose_service: String,
    /// Database name (not a secret).
    database: Option<String>,
    /// Name of the environment variable inside the container that holds the user.
    user_env: Option<String>,
    /// Name of the environment variable inside the container that holds the password.
    password_env: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SetArgs {
    id: String,
    databases: Vec<DatabaseArgs>,
}
