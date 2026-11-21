//! Deploy Keys and Registry Credentials. Private halves and secrets are write-only.

use schemars::JsonSchema;
use serde::Deserialize;
use yanto_core::actor::Scope;
use yanto_core::secrets::Secret;
use yanto_core::service::registry_credentials::CredentialUpdate;

use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, done, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_deploy_key_list",
        "deploy_key_list",
        "List Deploy Keys (public data only).",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.deploy_keys.list(&a).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_deploy_key_public_get",
        "deploy_key_public_get",
        "Get a Deploy Key's public half.",
        Scope::Read,
        Class::Read,
        |s, a, args: IdArgs| async move { to_json(&s.deploy_keys.public_get(&a, &args.id).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_deploy_key_create",
            "deploy_key_create",
            "Generate a new SSH Deploy Key. Only the public half is returned.",
            Scope::Admin,
            Class::Write,
            |s, a, args: NameArgs| async move { to_json(&s.deploy_keys.create(&a, &args.name).await?) },
        )
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_deploy_key_delete",
            "deploy_key_delete",
            "Delete a Deploy Key.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move {
                s.deploy_keys.delete(&a, &args.id).await?;
                done()
            },
        )
        .confirm(),
    );
    set.add(ToolDef::new(
        "yanto_registry_credential_list",
        "registry_credential_list",
        "List credentials for pulling images from private registries (secret never shown).",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.registry_credentials.list(&a).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_registry_credential_create",
            "registry_credential_create",
            "Store a registry credential (the secret is write-only).",
            Scope::Admin,
            Class::Write,
            |s, a, args: CredCreate| async move {
                to_json(
                    &s.registry_credentials
                        .create(
                            &a,
                            &args.name,
                            &args.registry_host,
                            &args.username,
                            &Secret::new(args.secret),
                        )
                        .await?,
                )
            },
        )
        .confirm()
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_registry_credential_update",
            "registry_credential_update",
            "Change a registry credential; absent fields keep their value.",
            Scope::Admin,
            Class::Write,
            |s, a, args: CredUpdate| async move {
                let change = CredentialUpdate {
                    name: args.name,
                    registry_host: args.registry_host,
                    username: args.username,
                    secret: args.secret.map(Secret::new),
                };
                to_json(&s.registry_credentials.update(&a, &args.id, &change).await?)
            },
        )
        .confirm()
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_registry_credential_delete",
            "registry_credential_delete",
            "Delete a registry credential.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move {
                s.registry_credentials.delete(&a, &args.id).await?;
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
struct NameArgs {
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CredCreate {
    name: String,
    registry_host: String,
    username: String,
    /// Password or access token.
    secret: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CredUpdate {
    id: String,
    name: Option<String>,
    registry_host: Option<String>,
    username: Option<String>,
    secret: Option<String>,
}
