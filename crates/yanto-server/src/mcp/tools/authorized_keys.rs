//! SSH `authorized_keys` management (Owner only).

use schemars::JsonSchema;
use serde::Deserialize;
use yanto_core::actor::Scope;
use yanto_core::service::authorized_keys::AddKey;

use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_authorized_keys_list",
        "authorized_keys_list",
        "List SSH keys in Yanto's marked block of authorized_keys.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.authorized_keys.list(&a).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_authorized_keys_add",
            "authorized_keys_add",
            "Add an SSH public key to authorized_keys.",
            Scope::Admin,
            Class::Write,
            |s, a, args: AddArgs| async move {
                to_json(
                    &s.authorized_keys
                        .add(
                            &a,
                            &AddKey {
                                name: args.name.unwrap_or_default(),
                                public_key: args.public_key,
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
            "yanto_authorized_keys_delete",
            "authorized_keys_delete",
            "Remove an SSH key. confirm_user (the Unix user name) overrides the lockout guard.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: DeleteArgs| async move {
                to_json(
                    &s.authorized_keys
                        .delete(&a, &args.id, args.confirm_user.as_deref())
                        .await?,
                )
            },
        )
        .confirm(),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddArgs {
    name: Option<String>,
    public_key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeleteArgs {
    id: String,
    confirm_user: Option<String>,
}
