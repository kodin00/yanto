//! Host Daemons (`cloudflared`, `traefik`, `frps`): read-only status. Starting, stopping and
//! restarting a daemon (`daemon_control`) is deliberately not offered over MCP.

use yanto_core::actor::Scope;

use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_daemons_status",
        "daemons_status",
        "Installed/running state, version and install hints of cloudflared, traefik and frps.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.daemons.status(&a).await?) },
    ));
    set.add(ToolDef::new(
        "yanto_traefik_status",
        "traefik_status",
        "Traefik daemon state, dynamic directory and whether Yanto's route file is in sync.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.hostnames.traefik_status(&a).await?) },
    ));
}
