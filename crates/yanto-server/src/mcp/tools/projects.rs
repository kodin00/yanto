//! Projects: CRUD, compose, env (always masked), ports, Deploy Token, stop/restart.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use yanto_core::CoreError;
use yanto_core::actor::Scope;
use yanto_core::deploy::env::VarChange;
use yanto_core::service::projects::{
    DeleteOptions, PortInput, ProjectInput, ProjectPatch, SourceInput,
};

use super::double_option;
use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, done, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_projects_list",
        "project_list",
        "List the Projects you can see.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.projects.list(&a).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_project_get",
        "project_get",
        "Get one Project by id.",
        Scope::Read,
        Class::Read,
        |s, a, args: IdArgs| async move { to_json(&s.projects.get(&a, &args.id).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_project_create",
        "project_create",
        "Create a Project from a git URL, an image reference or one free-form source. The Deploy Token is NOT returned; reveal it with yanto_project_deploy_token_reveal.",
        Scope::Write,
        Class::Write,
        |s, a, args: CreateArgs| async move {
            let source = match (args.git_url, args.image_ref, args.source) {
                (Some(url), None, None) => SourceInput::Git {
                    url,
                    branch: args.branch,
                },
                (None, Some(image_ref), None) => SourceInput::Image { image_ref },
                (None, None, Some(text)) => SourceInput::Auto(text),
                _ => {
                    return Err(CoreError::validation(
                        "source",
                        "give exactly one of git_url, image_ref or source",
                    ));
                }
            };
            let out = s
                .projects
                .create(
                    &a,
                    &ProjectInput {
                        name: args.name,
                        slug: args.slug,
                        source,
                        compose_path: args.compose_path,
                        compose_content: args.compose_content,
                        env_file: args.env_file,
                        auto_start: args.auto_start,
                        manual_deploy_enabled: args.manual_deploy_enabled,
                        github_webhook_enabled: args.github_webhook_enabled,
                        deploy_key_id: args.deploy_key_id,
                        generate_deploy_key: args.generate_deploy_key,
                        registry_credential_id: args.registry_credential_id,
                    },
                )
                .await?;
            // The Deploy Token is redacted over MCP (03 d); `out.deploy_token` is dropped here.
            Ok(json!({
                "project": to_json(&out.project)?,
                "deploy_key": to_json(&out.deploy_key)?,
                "deploy_token": "redacted; call yanto_project_deploy_token_reveal",
            }))
        },
    ));
    set.add(ToolDef::new(
        "yanto_project_update",
        "project_update",
        "Change a Project's settings. Absent fields keep their value; null clears compose_content, deploy_key_id or registry_credential_id.",
        Scope::Write,
        Class::Write,
        |s, a, args: UpdateArgs| async move {
            let patch = ProjectPatch {
                name: args.name,
                git_url: args.git_url,
                branch: args.branch,
                image_ref: args.image_ref,
                compose_path: args.compose_path,
                compose_content: args.compose_content,
                env_file: args.env_file,
                auto_start: args.auto_start,
                manual_deploy_enabled: args.manual_deploy_enabled,
                github_webhook_enabled: args.github_webhook_enabled,
                deploy_key_id: args.deploy_key_id,
                registry_credential_id: args.registry_credential_id,
            };
            to_json(&s.projects.update(&a, &args.id, &patch).await?)
        },
    ));
    set.add(
        ToolDef::new(
            "yanto_project_delete",
            "project_delete",
            "Delete a Project. Needs confirm_slug equal to the Project's slug; optionally stops containers and deletes volumes.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: DeleteArgs| async move {
                s.projects
                    .delete(
                        &a,
                        &args.id,
                        &DeleteOptions {
                            stop_containers: args.stop_containers,
                            delete_volumes: args.delete_volumes,
                            confirm_slug: args.confirm_slug,
                        },
                    )
                    .await?;
                done()
            },
        )
        .confirm(),
    );
    set.add(ToolDef::new(
        "yanto_project_compose_read",
        "project_compose_read",
        "Read the Project's compose file.",
        Scope::Read,
        Class::Read,
        |s, a, args: IdArgs| async move { to_json(&s.projects.compose_read(&a, &args.id).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_project_env_read",
            "project_env_read",
            "Read the Project's environment variables. Values are masked.",
            Scope::Read,
            Class::Read,
            |s, a, args: IdArgs| async move { to_json(&s.projects.env_read(&a, &args.id).await?) },
        )
        .untrusted(),
    );
    set.add(
        ToolDef::new(
            "yanto_project_env_content_read",
            "project_env_content_read",
            "Read the Project's .env text. Values are always masked over MCP.",
            Scope::Read,
            Class::Read,
            |s, a, args: IdArgs| async move {
                // Never unmasked over MCP (03 d prompt-injection note).
                let text = s.projects.env_content_read(&a, &args.id, false).await?;
                Ok(json!({ "content": text, "masked": true }))
            },
        )
        .untrusted(),
    );
    set.add(
        ToolDef::new(
            "yanto_project_env_preview",
            "project_env_preview",
            "Parse .env text and show the variables (masked) without saving anything.",
            Scope::Read,
            Class::Read,
            |s, a, args: ContentArgs| async move {
                to_json(&s.projects.env_preview(&a, None, &args.content).await?)
            },
        )
        .untrusted(),
    );
    set.add(ToolDef::new(
        "yanto_project_env_write",
        "project_env_write",
        "Replace the Project's whole .env text. Applied by the next deploy.",
        Scope::Write,
        Class::Write,
        |s, a, args: EnvWriteArgs| async move {
            to_json(&s.projects.env_write(&a, &args.id, &args.content).await?)
        },
    ));
    set.add(ToolDef::new(
        "yanto_project_env_variables_write",
        "project_env_variables_write",
        "Change individual variables: value null deletes, \"********\" keeps the stored value.",
        Scope::Write,
        Class::Write,
        |s, a, args: VarsArgs| async move {
            let changes: Vec<VarChange> = args
                .variables
                .into_iter()
                .map(|v| VarChange {
                    key: v.key,
                    value: v.value,
                })
                .collect();
            to_json(
                &s.projects
                    .env_variables_write(&a, &args.id, &changes)
                    .await?,
            )
        },
    ));
    set.add(ToolDef::new(
        "yanto_project_ports_list",
        "project_ports_list",
        "List the Project's host port bindings.",
        Scope::Read,
        Class::Read,
        |s, a, args: IdArgs| async move { to_json(&s.projects.ports_list(&a, &args.id).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_project_ports_set",
        "project_ports_set",
        "Replace the Project's host port bindings.",
        Scope::Write,
        Class::Write,
        |s, a, args: PortsArgs| async move {
            let ports: Vec<PortInput> = args
                .ports
                .into_iter()
                .map(|p| PortInput {
                    label: p.label,
                    compose_service: p.compose_service,
                    host_port: p.host_port,
                    container_port: p.container_port,
                    bind_address: p.bind_address.unwrap_or_else(|| "127.0.0.1".into()),
                    protocol: p.protocol.unwrap_or_else(|| "tcp".into()),
                })
                .collect();
            to_json(&s.projects.ports_set(&a, &args.id, &ports).await?)
        },
    ));
    set.add(
        ToolDef::new(
            "yanto_project_deploy_token_reveal",
            "project_deploy_token_reveal",
            "Reveal the Project's Deploy Token (a live credential).",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move {
                let t = s.projects.deploy_token_reveal(&a, &args.id).await?;
                Ok(json!({ "deploy_token": t.expose() }))
            },
        )
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_project_deploy_token_rotate",
            "project_deploy_token_rotate",
            "Rotate the Project's Deploy Token; the old one stops working.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move {
                let t = s.projects.deploy_token_rotate(&a, &args.id).await?;
                Ok(json!({ "deploy_token": t.expose() }))
            },
        )
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_project_stop",
            "project_stop",
            "Stop the Project's compose stack.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move { to_json(&s.projects.stop(&a, &args.id).await?) },
        )
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_project_restart",
            "project_restart",
            "Restart the Project's compose stack.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move { to_json(&s.projects.restart(&a, &args.id).await?) },
        )
        .confirm(),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct IdArgs {
    /// Project id.
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ContentArgs {
    /// `.env` text.
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EnvWriteArgs {
    /// Project id.
    id: String,
    /// New `.env` text.
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreateArgs {
    name: String,
    slug: Option<String>,
    git_url: Option<String>,
    branch: Option<String>,
    image_ref: Option<String>,
    /// One free-form box instead of git_url / image_ref.
    source: Option<String>,
    compose_path: Option<String>,
    compose_content: Option<String>,
    env_file: Option<String>,
    auto_start: Option<bool>,
    manual_deploy_enabled: Option<bool>,
    github_webhook_enabled: Option<bool>,
    deploy_key_id: Option<String>,
    #[serde(default)]
    generate_deploy_key: bool,
    registry_credential_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateArgs {
    /// Project id.
    id: String,
    name: Option<String>,
    git_url: Option<String>,
    branch: Option<String>,
    image_ref: Option<String>,
    compose_path: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    compose_content: Option<Option<String>>,
    env_file: Option<String>,
    auto_start: Option<bool>,
    manual_deploy_enabled: Option<bool>,
    github_webhook_enabled: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    deploy_key_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    registry_credential_id: Option<Option<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeleteArgs {
    /// Project id.
    id: String,
    #[serde(default)]
    stop_containers: bool,
    #[serde(default)]
    delete_volumes: bool,
    /// The Project's slug, typed back as a second guard.
    confirm_slug: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct VarArg {
    key: String,
    /// null deletes the variable; "********" keeps the stored value.
    value: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct VarsArgs {
    /// Project id.
    id: String,
    variables: Vec<VarArg>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PortArg {
    label: String,
    compose_service: Option<String>,
    host_port: u16,
    container_port: u16,
    /// Default 127.0.0.1.
    bind_address: Option<String>,
    /// Default tcp.
    protocol: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PortsArgs {
    /// Project id.
    id: String,
    ports: Vec<PortArg>,
}
