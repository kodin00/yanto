//! Invocations for the M7 operations in the permission matrix (rows live in `matrix.rs`).

use crate::actor::Actor;
use crate::error::CoreError;
use crate::ingress::frps::Patch;
use crate::secrets::Secret;
use crate::service::cloudflare::DnsInput;
use crate::service::frps::FrpSettingsInput;
use crate::service::hostnames::{HostnameInput, TargetInput};
use crate::testing::TestCore;
use crate::testing::ingress::CF_ACCOUNT;

/// A Traefik Hostname on `prj_1`, created through the service as the CLI. Idempotent.
pub(crate) async fn seed_hostname(core: &TestCore) -> String {
    let cli = Actor::cli();
    let s = &core.services.hostnames;
    if let Some(h) = s
        .list(&cli, None)
        .await
        .unwrap()
        .into_iter()
        .find(|h| h.hostname == "seed.example.com")
    {
        return h.id;
    }
    core.enable_traefik();
    let port = core.ensure_port("prj_1", 18080).await;
    s.create(
        &cli,
        &HostnameInput {
            target: TargetInput::Project {
                project_id: "prj_1".into(),
                project_port_id: port,
            },
            hostname: "seed.example.com".into(),
            ingress_provider: "traefik".into(),
            tunnel_id: None,
            zone_id: None,
            manage_dns: false,
        },
    )
    .await
    .expect("seed hostname")
    .hostname
    .id
}

/// `None` when `op` is not an M7 operation.
pub(crate) async fn invoke(core: &TestCore, op: &str, a: &Actor) -> Option<Result<(), CoreError>> {
    let s = &core.services;
    let r = match op {
        "daemons_status" => s.daemons.status(a).await.map(|_| ()),
        "daemon_control" => s
            .daemons
            .control(a, "cloudflared", "start", true)
            .await
            .map(|_| ()),
        "cloudflare_clients_list" => s.cloudflare.clients_list(a).await.map(|_| ()),
        "cloudflare_client_validate" => s
            .cloudflare
            .client_validate(a, CF_ACCOUNT, &Secret::new("x"))
            .await
            .map(|_| ()),
        "cloudflare_client_create" => s
            .cloudflare
            .client_create(a, "matrix", CF_ACCOUNT, &Secret::new("x"))
            .await
            .map(|_| ()),
        "cloudflare_client_update" => s
            .cloudflare
            .client_update(a, "cfc_missing", Some("n"), None)
            .await
            .map(|_| ()),
        "cloudflare_client_delete" => s.cloudflare.client_delete(a, "cfc_missing").await,
        "cloudflare_zones_list" => s
            .cloudflare
            .zones_list(a, "cfc_missing", false)
            .await
            .map(|_| ()),
        "cloudflare_tunnels_list" => s.cloudflare.tunnels_list(a).await.map(|_| ()),
        "cloudflare_tunnel_create" => s
            .cloudflare
            .tunnel_create(a, "cfc_missing", "t")
            .await
            .map(|_| ()),
        "cloudflare_tunnel_activate" => s.cloudflare.tunnel_activate(a, "cft_missing").await,
        "cloudflare_tunnel_status" => s
            .cloudflare
            .tunnel_status(a, "cft_missing")
            .await
            .map(|_| ()),
        "cloudflare_tunnel_delete" => s.cloudflare.tunnel_delete(a, "cft_missing", false).await,
        "cloudflare_tunnel_token_reveal" => s
            .cloudflare
            .tunnel_token_reveal(a, "cft_missing")
            .await
            .map(|_| ()),
        "cloudflare_dns_list" => s
            .cloudflare
            .dns_list(a, "cfc_missing", "zone-1", false)
            .await
            .map(|_| ()),
        "cloudflare_dns_create" => s
            .cloudflare
            .dns_create(a, "cfc_missing", "zone-1", &DnsInput::default())
            .await
            .map(|_| ()),
        "cloudflare_dns_update" => s
            .cloudflare
            .dns_update(a, "cfc_missing", "zone-1", "rec", &DnsInput::default())
            .await
            .map(|_| ()),
        "cloudflare_dns_delete" => {
            s.cloudflare
                .dns_delete(a, "cfc_missing", "zone-1", "rec")
                .await
        }
        "frps_overview" => s.frps.overview(a).await.map(|_| ()),
        "frps_settings_update" => s
            .frps
            .settings_update(
                a,
                &FrpSettingsInput {
                    patch: Patch::default(),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ()),
        "frps_config_restore" => s.frps.config_restore(a).await,
        "frps_token_reveal" => s.frps.token_reveal(a).await.map(|_| ()),
        "traefik_status" => s.hostnames.traefik_status(a).await.map(|_| ()),
        "hostnames_resync" => s.hostnames.resync(a).await.map(|_| ()),
        "hostnames_public_ip_set" => {
            s.hostnames
                .public_ip_set(a, Some("203.0.113.5"), None)
                .await
        }
        "hostname_registry_manage" => s
            .hostnames
            .create(
                a,
                &HostnameInput {
                    target: TargetInput::Registry,
                    hostname: "reg.example.com".into(),
                    ingress_provider: "traefik".into(),
                    tunnel_id: None,
                    zone_id: None,
                    manage_dns: false,
                },
            )
            .await
            .map(|_| ()),
        "hostnames_list" => s.hostnames.list(a, None).await.map(|_| ()),
        "hostnames_options" => s.hostnames.options(a).await.map(|_| ()),
        "hostnames_diagnostics" => s.hostnames.diagnostics(a, None, false).await.map(|_| ()),
        "hostname_create" => {
            core.enable_traefik();
            let port = core.ensure_port("prj_1", 18081).await;
            s.hostnames
                .create(
                    a,
                    &HostnameInput {
                        target: TargetInput::Project {
                            project_id: "prj_1".into(),
                            project_port_id: port,
                        },
                        hostname: "matrix.example.com".into(),
                        ingress_provider: "traefik".into(),
                        tunnel_id: None,
                        zone_id: None,
                        manage_dns: false,
                    },
                )
                .await
                .map(|_| ())
        }
        "hostname_retry" => {
            let id = seed_hostname(core).await;
            s.hostnames.retry(a, &id).await.map(|_| ())
        }
        "hostname_enable" => {
            let id = seed_hostname(core).await;
            s.hostnames.enable(a, &id).await.map(|_| ())
        }
        "hostname_disable" => {
            let id = seed_hostname(core).await;
            s.hostnames.disable(a, &id).await.map(|_| ())
        }
        "hostname_delete" => {
            let id = seed_hostname(core).await;
            s.hostnames.delete(a, &id).await.map(|_| ())
        }
        _ => return None,
    };
    Some(r)
}
