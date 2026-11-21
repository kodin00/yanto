//! Restore a Project Backup (volume archive or database dump). Owner only; the service takes a
//! `pre_restore` backup first and verifies the checksum before anything changes.

use schemars::JsonSchema;
use serde::Deserialize;
use yanto_core::actor::Scope;
use yanto_core::service::backups::RestoreOptions;

use crate::mcp::{Class, ToolDef, ToolSet, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(
        ToolDef::new(
            "yanto_backup_restore",
            "backup_restore",
            "Restore a Backup. Volumes go to a new `<volume>-restore-<ts>` volume unless in_place is true; in-place and database restores need confirm_name = the Project name.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: RestoreArgs| async move {
                to_json(
                    &s.backups
                        .restore(
                            &a,
                            &args.id,
                            RestoreOptions {
                                in_place: args.in_place,
                                confirm_name: args.confirm_name,
                            },
                        )
                        .await?,
                )
            },
        )
        .confirm(),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RestoreArgs {
    /// Backup id.
    id: String,
    /// Empty the original volume instead of restoring beside it.
    #[serde(default)]
    in_place: bool,
    /// The Project name, typed; required for in-place volume and database restores.
    confirm_name: Option<String>,
}
