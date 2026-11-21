//! Cron Entries (Owner only). `cron_run` stays CLI-only.

use schemars::JsonSchema;
use serde::Deserialize;
use yanto_core::CoreError;
use yanto_core::actor::Scope;
use yanto_core::cron::{EntryInput, Overlap};
use yanto_core::service::cron::CronPatch;

use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, done, to_json};

fn overlap(s: &str) -> Result<Overlap, CoreError> {
    Overlap::parse(s)
        .ok_or_else(|| CoreError::validation("overlap", "overlap must be allow or skip"))
}

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_cron_list",
        "cron_list",
        "List Cron Entries and file drift status.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.cron.list(&a).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_cron_files_list",
            "cron_files_list",
            "List system cron files (read-only).",
            Scope::Read,
            Class::Read,
            |s, a, _: NoArgs| async move { to_json(&s.cron.files_list(&a).await?) },
        )
        .untrusted(),
    );
    set.add(
        ToolDef::new(
            "yanto_cron_runs_list",
            "cron_runs_list",
            "Recent runs of a Cron Entry (exit code, duration, output tail).",
            Scope::Read,
            Class::Read,
            |s, a, args: IdArgs| async move { to_json(&s.cron.runs_list(&a, &args.id).await?) },
        )
        .untrusted(),
    );
    set.add(
        ToolDef::new(
            "yanto_cron_create",
            "cron_create",
            "Create a Cron Entry that runs a command as a Unix user on a schedule.",
            Scope::Admin,
            Class::Write,
            |s, a, args: CreateArgs| async move {
                let input = EntryInput {
                    name: args.name,
                    schedule: args.schedule,
                    run_as_user: args.run_as_user,
                    command: args.command,
                    overlap: overlap(args.overlap.as_deref().unwrap_or("allow"))?,
                    timeout_s: args.timeout_s,
                    enabled: args.enabled.unwrap_or(true),
                };
                to_json(&s.cron.create(&a, &input).await?)
            },
        )
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_cron_update",
            "cron_update",
            "Change a Cron Entry; absent fields keep their value.",
            Scope::Admin,
            Class::Write,
            |s, a, args: UpdateArgs| async move {
                let patch = CronPatch {
                    name: args.name,
                    schedule: args.schedule,
                    run_as_user: args.run_as_user,
                    command: args.command,
                    overlap: args.overlap.as_deref().map(overlap).transpose()?,
                    timeout_s: args.timeout_s,
                    enabled: args.enabled,
                };
                to_json(&s.cron.patch(&a, &args.id, &patch).await?)
            },
        )
        .confirm(),
    );
    set.add(ToolDef::new(
        "yanto_cron_toggle",
        "cron_toggle",
        "Enable or disable a Cron Entry.",
        Scope::Write,
        Class::Write,
        |s, a, args: ToggleArgs| async move {
            to_json(&s.cron.toggle(&a, &args.id, args.enabled).await?)
        },
    ));
    set.add(
        ToolDef::new(
            "yanto_cron_delete",
            "cron_delete",
            "Delete a Cron Entry.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move {
                s.cron.delete(&a, &args.id).await?;
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
struct ToggleArgs {
    id: String,
    enabled: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreateArgs {
    name: String,
    /// Five-field cron expression.
    schedule: String,
    run_as_user: String,
    command: String,
    /// allow (default) or skip when the previous run is still going.
    overlap: Option<String>,
    timeout_s: Option<i64>,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateArgs {
    id: String,
    name: Option<String>,
    schedule: Option<String>,
    run_as_user: Option<String>,
    command: Option<String>,
    overlap: Option<String>,
    timeout_s: Option<i64>,
    enabled: Option<bool>,
}
