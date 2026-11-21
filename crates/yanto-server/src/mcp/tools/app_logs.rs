//! The Yanto application log (secrets are masked when a line is recorded).

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use yanto_core::actor::Scope;

use crate::mcp::{Class, ToolDef, ToolSet, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(
        ToolDef::new(
            "yanto_system_logs",
            "system_logs",
            "Recent lines of Yanto's own application log, oldest first (default and maximum 500).",
            Scope::Read,
            Class::Read,
            |s, a, args: LogArgs| async move {
                Ok(json!({ "lines": to_json(&s.system.logs(&a, args.tail).await?)? }))
            },
        )
        .untrusted(),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LogArgs {
    /// Number of newest lines.
    tail: Option<usize>,
}
