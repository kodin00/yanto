//! Hostnames: list, create, retry, enable, disable, delete, diagnostics. Hostnames of the
//! Hosted Registry (Owner only) are managed from the dashboard, not here.

use schemars::JsonSchema;
use serde::Deserialize;
use yanto_core::actor::Scope;
use yanto_core::service::hostnames::{HostnameInput, TargetInput};

use crate::mcp::{Class, ToolDef, ToolSet, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_hostnames_list",
        "hostnames_list",
        "List Hostnames you can see, optionally for one Project.",
        Scope::Read,
        Class::Read,
        |s, a, args: ListArgs| async move {
            to_json(&s.hostnames.list(&a, args.project_id.as_deref()).await?)
        },
    ));
    set.add(
        ToolDef::new(
            "yanto_hostname_create",
            "hostname_create",
            "Publish a Project port under a public hostname (Cloudflare tunnel or Traefik).",
            Scope::Write,
            Class::Write,
            |s, a, args: CreateArgs| async move {
                let input = HostnameInput {
                    target: TargetInput::Project {
                        project_id: args.project_id,
                        project_port_id: args.project_port_id,
                    },
                    hostname: args.hostname,
                    ingress_provider: args.ingress_provider,
                    tunnel_id: args.tunnel_id,
                    zone_id: args.zone_id,
                    manage_dns: args.manage_dns,
                };
                to_json(&s.hostnames.create(&a, &input).await?)
            },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_hostname_retry",
            "hostname_retry",
            "Re-run the publish of a Hostname.",
            Scope::Write,
            Class::Write,
            |s, a, args: IdArgs| async move { to_json(&s.hostnames.retry(&a, &args.id).await?) },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_hostname_enable",
            "hostname_enable",
            "Enable a Hostname and publish it.",
            Scope::Write,
            Class::Write,
            |s, a, args: IdArgs| async move { to_json(&s.hostnames.enable(&a, &args.id).await?) },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_hostname_disable",
            "hostname_disable",
            "Disable a Hostname and withdraw its route.",
            Scope::Write,
            Class::Write,
            |s, a, args: IdArgs| async move { to_json(&s.hostnames.disable(&a, &args.id).await?) },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_hostname_delete",
            "hostname_delete",
            "Delete a Hostname, its route and the DNS record Yanto created. Returns warnings.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move { to_json(&s.hostnames.delete(&a, &args.id).await?) },
        )
        .open_world()
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_hostnames_diagnostics",
            "hostnames_diagnostics",
            "Read-only DNS, tunnel, Traefik and (optionally) public reachability checks per Hostname.",
            Scope::Read,
            Class::Read,
            |s, a, args: DiagArgs| async move {
                to_json(
                    &s.hostnames
                        .diagnostics(&a, args.hostname_id.as_deref(), args.public)
                        .await?,
                )
            },
        )
        .open_world(),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct IdArgs {
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListArgs {
    /// Only Hostnames of this Project.
    project_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DiagArgs {
    /// Check one Hostname; omit for all you can see.
    hostname_id: Option<String>,
    /// Also send `HEAD https://<hostname>` (opt-in).
    #[serde(default)]
    public: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreateArgs {
    project_id: String,
    /// A `ppt_` id from yanto_project_ports_list.
    project_port_id: String,
    hostname: String,
    /// `cloudflare_tunnel` or `traefik`.
    ingress_provider: String,
    /// Required for `cloudflare_tunnel` (a `cft_` id); omit to use the active tunnel.
    tunnel_id: Option<String>,
    zone_id: Option<String>,
    /// Create and own the DNS record (Cloudflare zone required).
    #[serde(default)]
    manage_dns: bool,
}
