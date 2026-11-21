//! frps: overview and settings. Saving the settings only writes `frps.toml` (with a backup);
//! applying them means restarting `frps`, which drops every live tunnel, so the tool is guarded
//! by `confirm: true`. The token and dashboard password are write-only.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use yanto_core::CoreError;
use yanto_core::actor::Scope;
use yanto_core::ingress::frps::{Change, Patch, PortRange};
use yanto_core::secrets::Secret;
use yanto_core::service::frps::FrpSettingsInput;

use super::double_option;
use crate::mcp::{Class, NoArgs, ToolDef, ToolSet, to_json};

pub fn register(set: &mut ToolSet) {
    set.add(ToolDef::new(
        "yanto_frps_overview",
        "frps_overview",
        "frps status, owned config fields (secrets shown only as set/unset) and live proxies.",
        Scope::Read,
        Class::Read,
        |s, a, _: NoArgs| async move { to_json(&s.frps.overview(&a).await?) },
    ));
    set.add(
        ToolDef::new(
            "yanto_frps_settings_update",
            "frps_settings_update",
            "Change the frps.toml fields Yanto owns. A field that is absent is kept, null clears it. The change takes effect when frps restarts, which drops every live tunnel.",
            Scope::Write,
            Class::Write,
            |s, a, args: SettingsArgs| async move {
                let allow = match args.allow_ports {
                    None => Change::Keep,
                    Some(None) => Change::Clear,
                    Some(Some(list)) => {
                        let mut out = Vec::new();
                        for r in list {
                            out.push(match (r.single, r.start, r.end) {
                                (Some(port), None, None) => PortRange::Single { port },
                                (None, Some(start), Some(end)) => PortRange::Range { start, end },
                                _ => {
                                    return Err(CoreError::validation(
                                        "allow_ports",
                                        "each entry is {single} or {start, end}",
                                    ));
                                }
                            });
                        }
                        Change::Set(out)
                    }
                };
                let input = FrpSettingsInput {
                    public_host: args.public_host,
                    generate_token: args.generate_token,
                    patch: Patch {
                        bind_addr: change(args.bind_addr),
                        bind_port: change(args.bind_port),
                        auth_method: change(args.auth_method),
                        auth_token: change(args.auth_token.map(|o| o.map(Secret::new))),
                        web_addr: change(args.web_addr),
                        web_port: change(args.web_port),
                        web_user: change(args.web_user),
                        web_password: change(args.web_password.map(|o| o.map(Secret::new))),
                        allow_ports: allow,
                    },
                };
                let out = s.frps.settings_update(&a, &input).await?;
                Ok(json!({
                    "changed": out.changed,
                    "diff": out.diff,
                    "pending_restart": out.pending_restart,
                    "token_generated": out.token_generated,
                }))
            },
        )
        .confirm(),
    );
}

fn change<T>(v: Option<Option<T>>) -> Change<T> {
    match v {
        None => Change::Keep,
        Some(None) => Change::Clear,
        Some(Some(x)) => Change::Set(x),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PortRangeArgs {
    /// One port.
    single: Option<u16>,
    /// A range, both ends set.
    start: Option<u16>,
    end: Option<u16>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SettingsArgs {
    /// Public host name shown to frpc users (stored in Yanto, not in frps.toml).
    public_host: Option<String>,
    /// Generate a new `auth.token` (never returned) and set `auth.method = "token"`.
    #[serde(default)]
    generate_token: bool,
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<String>")]
    bind_addr: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<u16>")]
    bind_port: Option<Option<u16>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<String>")]
    auth_method: Option<Option<String>>,
    /// Write-only.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<String>")]
    auth_token: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<String>")]
    web_addr: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<u16>")]
    web_port: Option<Option<u16>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<String>")]
    web_user: Option<Option<String>>,
    /// Write-only.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<String>")]
    web_password: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<Vec<PortRangeArgs>>")]
    allow_ports: Option<Option<Vec<PortRangeArgs>>>,
}
