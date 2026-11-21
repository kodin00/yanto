//! Cloudflare: clients (API tokens are write-only), zones, tunnels and DNS records. The tunnel
//! connector token is never returned over MCP.

use schemars::JsonSchema;
use serde::Deserialize;
use yanto_core::actor::Scope;
use yanto_core::secrets::Secret;
use yanto_core::service::cloudflare::DnsInput;

use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, done, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_cloudflare_clients_list",
        "cloudflare_clients_list",
        "List configured Cloudflare clients (account id and name; tokens are never returned).",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.cloudflare.clients_list(&a).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_cloudflare_client_validate",
            "cloudflare_client_validate",
            "Check a Cloudflare API token against the account without saving it.",
            Scope::Admin,
            Class::Read,
            |s, a, args: ValidateArgs| async move {
                to_json(
                    &s.cloudflare
                        .client_validate(&a, &args.account_id, &Secret::new(args.api_token))
                        .await?,
                )
            },
        )
        .open_world()
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_client_create",
            "cloudflare_client_create",
            "Add a Cloudflare client (the token is validated and stored encrypted).",
            Scope::Admin,
            Class::Write,
            |s, a, args: ClientCreateArgs| async move {
                to_json(
                    &s.cloudflare
                        .client_create(
                            &a,
                            &args.name,
                            &args.account_id,
                            &Secret::new(args.api_token),
                        )
                        .await?,
                )
            },
        )
        .open_world()
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_client_update",
            "cloudflare_client_update",
            "Rename a Cloudflare client and/or replace its API token.",
            Scope::Admin,
            Class::Write,
            |s, a, args: ClientUpdateArgs| async move {
                let token = args.api_token.map(Secret::new);
                to_json(
                    &s.cloudflare
                        .client_update(&a, &args.id, args.name.as_deref(), token.as_ref())
                        .await?,
                )
            },
        )
        .open_world()
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_client_delete",
            "cloudflare_client_delete",
            "Delete a Cloudflare client.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: IdArgs| async move {
                s.cloudflare.client_delete(&a, &args.id).await?;
                done()
            },
        )
        .open_world()
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_zones_list",
            "cloudflare_zones_list",
            "List the zones of a Cloudflare client (cached; refresh=true asks Cloudflare).",
            Scope::Read,
            Class::Read,
            |s, a, args: ZonesArgs| async move {
                to_json(
                    &s.cloudflare
                        .zones_list(&a, &args.client_id, args.refresh)
                        .await?,
                )
            },
        )
        .open_world(),
    );
    set.add(ToolDef::new(
        "yanto_cloudflare_tunnels_list",
        "cloudflare_tunnels_list",
        "List Cloudflare tunnels known to Yanto.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.cloudflare.tunnels_list(&a).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_cloudflare_tunnel_create",
            "cloudflare_tunnel_create",
            "Create a remote-managed tunnel. The connector token is not returned: reveal it in the dashboard.",
            Scope::Write,
            Class::Write,
            |s, a, args: TunnelCreateArgs| async move {
                to_json(
                    &s.cloudflare
                        .tunnel_create(&a, &args.client_id, &args.name)
                        .await?,
                )
            },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_tunnel_activate",
            "cloudflare_tunnel_activate",
            "Mark a tunnel as the one bound to the host `cloudflared`.",
            Scope::Admin,
            Class::Write,
            |s, a, args: IdArgs| async move {
                s.cloudflare.tunnel_activate(&a, &args.id).await?;
                done()
            },
        )
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_tunnel_status",
            "cloudflare_tunnel_status",
            "Cloudflare-side health of a tunnel plus the local daemon state.",
            Scope::Read,
            Class::Read,
            |s, a, args: IdArgs| async move {
                to_json(&s.cloudflare.tunnel_status(&a, &args.id).await?)
            },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_tunnel_delete",
            "cloudflare_tunnel_delete",
            "Delete a tunnel; force=true also removes it while Hostnames still use it.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: TunnelDeleteArgs| async move {
                s.cloudflare.tunnel_delete(&a, &args.id, args.force).await?;
                done()
            },
        )
        .open_world()
        .confirm(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_dns_list",
            "cloudflare_dns_list",
            "List DNS records of a zone (cached; refresh=true asks Cloudflare).",
            Scope::Read,
            Class::Read,
            |s, a, args: DnsListArgs| async move {
                to_json(
                    &s.cloudflare
                        .dns_list(&a, &args.client_id, &args.zone_id, args.refresh)
                        .await?,
                )
            },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_dns_create",
            "cloudflare_dns_create",
            "Create a DNS record in a zone.",
            Scope::Write,
            Class::Write,
            |s, a, args: DnsCreateArgs| async move {
                to_json(
                    &s.cloudflare
                        .dns_create(&a, &args.client_id, &args.zone_id, &args.record.into())
                        .await?,
                )
            },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_dns_update",
            "cloudflare_dns_update",
            "Replace a DNS record (records bound to a Hostname are edited through the Hostname).",
            Scope::Write,
            Class::Write,
            |s, a, args: DnsUpdateArgs| async move {
                to_json(
                    &s.cloudflare
                        .dns_update(
                            &a,
                            &args.client_id,
                            &args.zone_id,
                            &args.record_id,
                            &args.record.into(),
                        )
                        .await?,
                )
            },
        )
        .open_world(),
    );
    set.add(
        ToolDef::new(
            "yanto_cloudflare_dns_delete",
            "cloudflare_dns_delete",
            "Delete a DNS record.",
            Scope::Admin,
            Class::Destructive,
            |s, a, args: DnsDeleteArgs| async move {
                s.cloudflare
                    .dns_delete(&a, &args.client_id, &args.zone_id, &args.record_id)
                    .await?;
                done()
            },
        )
        .open_world()
        .confirm(),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct IdArgs {
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ValidateArgs {
    account_id: String,
    /// Cloudflare API token (write-only; never echoed).
    api_token: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ClientCreateArgs {
    name: String,
    account_id: String,
    /// Cloudflare API token (write-only; never echoed).
    api_token: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ClientUpdateArgs {
    id: String,
    name: Option<String>,
    /// New Cloudflare API token (write-only; never echoed).
    api_token: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ZonesArgs {
    client_id: String,
    #[serde(default)]
    refresh: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TunnelCreateArgs {
    client_id: String,
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TunnelDeleteArgs {
    id: String,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DnsListArgs {
    client_id: String,
    zone_id: String,
    #[serde(default)]
    refresh: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DnsRecordArgs {
    /// A, AAAA, CNAME, TXT, ...
    record_type: String,
    name: String,
    content: String,
    /// Seconds; 1 means automatic.
    ttl: Option<i64>,
    #[serde(default)]
    proxied: bool,
    priority: Option<i64>,
    comment: Option<String>,
}

impl From<DnsRecordArgs> for DnsInput {
    fn from(r: DnsRecordArgs) -> Self {
        DnsInput {
            record_type: r.record_type,
            name: r.name,
            content: r.content,
            ttl: r.ttl,
            proxied: r.proxied,
            priority: r.priority,
            comment: r.comment,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DnsCreateArgs {
    client_id: String,
    zone_id: String,
    #[serde(flatten)]
    record: DnsRecordArgs,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DnsUpdateArgs {
    client_id: String,
    zone_id: String,
    record_id: String,
    #[serde(flatten)]
    record: DnsRecordArgs,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DnsDeleteArgs {
    client_id: String,
    zone_id: String,
    record_id: String,
}
