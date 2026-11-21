//! The Hosted Registry: status, enable/disable, users, repositories, tags, garbage collection.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use yanto_core::actor::Scope;
use yanto_core::secrets::Secret;
use yanto_core::service::registry::EnableInput;

use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, done, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_registry_status",
        "registry_status",
        "Hosted Registry status: enabled, port, container state, health.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.registry.status(&a).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_registry_enable",
            "registry_enable",
            "Enable the Hosted Registry (starts the registry container).",
            Scope::Admin,
            Class::Write,
            |s, a, args: EnableArgs| async move {
                to_json(
                    &s.registry
                        .enable(
                            &a,
                            &EnableInput {
                                listen_port: args.listen_port,
                            },
                        )
                        .await?,
                )
            },
        )
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_registry_disable",
            "registry_disable",
            "Disable the Hosted Registry. Stored images are kept unless delete_data is the phrase `delete registry data`.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: DisableArgs| async move {
                to_json(&s.registry.disable(&a, args.delete_data.as_deref()).await?)
            },
        )
        .confirm(),
    );
    set.add(ToolDef::new(
        "yanto_registry_users_list",
        "registry_users_list",
        "List Hosted Registry users.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move {
            Ok(json!({ "users": to_json(&s.registry.users_list(&a).await?)? }))
        },
    ));
    set.add(ToolDef::new(
        "yanto_registry_user_create",
        "registry_user_create",
        "Create a Hosted Registry user. A generated password is NOT returned over MCP: pass `password` yourself, or set one in the dashboard.",
        Scope::Write,
        Class::Write,
        |s, a, args: UserCreateArgs| async move {
            let pw = args.password.map(Secret::new);
            let out = s
                .registry
                .user_create(&a, &args.username, pw.as_ref())
                .await?;
            // A generated password is redacted over MCP (03 d); it is dropped here.
            Ok(json!({
                "user": to_json(&out.user)?,
                "password": if out.generated_password.is_some() {
                    "generated and redacted; set a new one in the dashboard"
                } else {
                    "as supplied"
                },
            }))
        },
    ));
    set.add(
        ToolDef::new(
            "yanto_registry_user_delete",
            "registry_user_delete",
            "Delete a Hosted Registry user.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move {
                s.registry.user_delete(&a, &args.id).await?;
                done()
            },
        )
        .confirm(),
    );
    set.add(ToolDef::new(
        "yanto_registry_repos_list",
        "registry_repos_list",
        "List repositories in the Hosted Registry (paged).",
        Scope::Read,
        Class::Read,
        |s, a, args: PageArgs| async move {
            to_json(
                &s.registry
                    .repos_list(&a, args.last.as_deref(), args.n)
                    .await?,
            )
        },
    ));
    set.add(ToolDef::new(
        "yanto_registry_tags_list",
        "registry_tags_list",
        "List tags of a repository in the Hosted Registry (paged).",
        Scope::Read,
        Class::Read,
        |s, a, args: TagsArgs| async move {
            to_json(
                &s.registry
                    .tags_list(&a, &args.repo, args.last.as_deref(), args.n)
                    .await?,
            )
        },
    ));
    set.add(
        ToolDef::new(
            "yanto_registry_tag_delete",
            "registry_tag_delete",
            "Delete a tag (and its digest) from the Hosted Registry.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: TagArgs| async move {
                to_json(&s.registry.tag_delete(&a, &args.repo, &args.tag).await?)
            },
        )
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_registry_gc",
            "registry_gc",
            "Start Hosted Registry garbage collection in the background (read-only window).",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: GcArgs| async move {
                to_json(&s.registry.gc_start(&a, args.delete_untagged).await?)
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
struct EnableArgs {
    listen_port: Option<u16>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DisableArgs {
    /// The phrase `delete registry data` also removes stored images.
    delete_data: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UserCreateArgs {
    username: String,
    password: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PageArgs {
    /// Last repository of the previous page.
    last: Option<String>,
    n: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TagsArgs {
    repo: String,
    last: Option<String>,
    n: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TagArgs {
    repo: String,
    tag: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GcArgs {
    #[serde(default)]
    delete_untagged: bool,
}
