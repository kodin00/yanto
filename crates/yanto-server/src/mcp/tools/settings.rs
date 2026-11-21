//! Runtime settings (Owner only). Host paths are `config.toml` and read-only.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use yanto_core::actor::Scope;

use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_settings_read",
        "settings_read",
        "Read the runtime settings (values, defaults, limits) and the read-only host configuration.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.settings.read(&a).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_settings_update",
            "settings_update",
            "Change runtime settings. `values` maps a setting key to its new value; null restores the default. Applies after Yanto restarts.",
            Scope::Admin,
            Class::Write,
            |s, a, args: UpdateArgs| async move { to_json(&s.settings.update(&a, &args.values).await?) },
        )
        .confirm(),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateArgs {
    values: BTreeMap<String, Value>,
}
