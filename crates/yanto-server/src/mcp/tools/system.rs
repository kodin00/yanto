//! `whoami`, `health`, `system_usage`, `audit_list`, `system_cleanup*`.

use schemars::JsonSchema;
use serde::Deserialize;
use yanto_core::actor::Scope;
use yanto_core::service::audit::AuditQuery;
use yanto_core::service::system::CleanupSelection;

use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_whoami",
        "whoami",
        "Show the user and API Token (scope, expiry) behind this connection.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.tokens.whoami(&a).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_health",
        "health",
        "Health report: database, Docker, disk and queue checks.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.system.health(&a).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_system_usage",
        "system_usage",
        "Host CPU, memory and disk usage plus Docker disk usage.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.system.usage(&a).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_audit_list",
        "audit_list",
        "List audit log entries, newest first, optionally for one Project. Page with the returned cursor.",
        Scope::Read,
        Class::Read,
        |s, a, args: AuditArgs| async move {
            let q = AuditQuery {
                project_id: args.project_id,
                limit: args.limit,
                cursor: args.cursor,
            };
            to_json(&s.audit.list(&a, &q).await?)
        },
    ));
    set.add(ToolDef::new(
        "yanto_cleanup_preview",
        "system_cleanup_preview",
        "Preview what a Docker cleanup would remove (build cache, dangling images, unused networks, stopped containers). Removes nothing.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.system.cleanup_preview(&a).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_cleanup",
            "system_cleanup",
            "Docker cleanup. Defaults: build cache, dangling images and unused networks on; stopped containers off.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: CleanupArgs| async move {
                let d = CleanupSelection::defaults();
                let sel = CleanupSelection {
                    build_cache: args.build_cache.unwrap_or(d.build_cache),
                    images: args.images.unwrap_or(d.images),
                    networks: args.networks.unwrap_or(d.networks),
                    containers: args.containers.unwrap_or(d.containers),
                    confirm: args.containers_confirm,
                };
                to_json(&s.system.cleanup(&a, &sel).await?)
            },
        )
        .confirm(),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AuditArgs {
    /// Only entries of this Project.
    project_id: Option<String>,
    /// Page size.
    limit: Option<i64>,
    /// `next_cursor` of the previous page.
    cursor: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CleanupArgs {
    build_cache: Option<bool>,
    images: Option<bool>,
    networks: Option<bool>,
    /// Remove stopped containers. Also needs `containers_confirm`.
    containers: Option<bool>,
    /// The phrase `prune containers`, required when `containers` is true.
    containers_confirm: Option<String>,
}
