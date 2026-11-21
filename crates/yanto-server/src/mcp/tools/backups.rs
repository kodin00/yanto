//! Backups: list, targets, create, delete. Download (binary) and restore are not exposed here.

use schemars::JsonSchema;
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::actor::Scope;
use yanto_core::backup::{BackupKind, Trigger};

use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, done, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_backups_list",
        "backups_list",
        "List Backups you can see.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.backups.list(&a).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_backup_targets_list",
        "backup_targets_list",
        "List what can be backed up (Yanto's database, Project volumes and databases).",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.backups.targets(&a).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_backup_create",
        "backup_create",
        "Create a Backup: yanto_sqlite (Owner), or project_volumes / project_postgres / project_mysql for a Project.",
        Scope::Write,
        Class::Write,
        |s, a, args: CreateArgs| async move {
            let kind = BackupKind::parse(&args.kind).ok_or_else(|| {
                CoreError::validation(
                    "kind",
                    "kind must be yanto_sqlite, project_volumes, project_postgres or project_mysql",
                )
            })?;
            let dto = if kind == BackupKind::YantoSqlite {
                s.backups.create_yanto(&a, Trigger::Api, None).await?
            } else {
                let project = args
                    .project_id
                    .ok_or_else(|| CoreError::validation("project_id", "project_id is required"))?;
                s.backups
                    .create_project(&a, &project, kind, args.target.as_deref(), args.stop_project)
                    .await?
            };
            to_json(&dto)
        },
    ));
    set.add(
        ToolDef::new(
            "yanto_backup_delete",
            "backup_delete",
            "Delete a Backup and its file.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move {
                s.backups.delete(&a, &args.id).await?;
                done()
            },
        )
        .confirm(),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct IdArgs {
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreateArgs {
    /// yanto_sqlite, project_volumes, project_postgres or project_mysql.
    kind: String,
    /// Required for Project kinds.
    project_id: Option<String>,
    /// Declared database service or volume name for Project kinds.
    target: Option<String>,
    /// Stop the Project while volumes are copied.
    #[serde(default)]
    stop_project: bool,
}
