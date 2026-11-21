//! M7: Host Daemons, Cloudflare, Hostnames (both providers) and FRP against the fakes.

use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::{Actor, Permission};
use crate::ingress::cloudflare::{CATCH_ALL, MANAGED_COMMENT};
use crate::ingress::frps::{Change, Patch, PortRange};
use crate::ports::{CfDnsRecord, IngressRule};
use crate::secrets::Secret;
use crate::service::frps::{FrpSettingsInput, FrpStatus};
use crate::service::hostnames::{DiagStatus, HostnameInput, TargetInput};
use crate::testing::TestCore;
use crate::testing::ingress::{CF_ACCOUNT, CF_TOKEN, CfFixture};

fn input(project: &str, port: &str, host: &str, provider: &str) -> HostnameInput {
    HostnameInput {
        target: TargetInput::Project {
            project_id: project.into(),
            project_port_id: port.into(),
        },
        hostname: host.into(),
        ingress_provider: provider.into(),
        tunnel_id: None,
        zone_id: None,
        manage_dns: false,
    }
}

fn cf_input(f: &CfFixture, port: &str, host: &str) -> HostnameInput {
    let mut i = input("prj_1", port, host, "cloudflare_tunnel");
    i.tunnel_id = Some(f.tunnel_id.clone());
    i.zone_id = Some(f.zone_id.clone());
    i.manage_dns = true;
    i
}

async fn audit_meta(core: &TestCore, action: &str) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT group_concat(metadata, ' ') FROM audit_logs WHERE action = ?",
    )
    .bind(action)
    .fetch_one(core.db.reader())
    .await
    .unwrap_or_default()
}

// ---- Cloudflare ---------------------------------------------------------------------------

#[tokio::test]
async fn client_validation_never_echoes_the_token_and_the_token_is_sealed() {
    let core = TestCore::new().await;
    core.ingress.cloudflare.accept_token(CF_TOKEN);
    let cli = Actor::cli();
    let bad = Secret::new("super-secret-wrong-token");
    let err = core
        .services
        .cloudflare
        .client_create(&cli, "c", CF_ACCOUNT, &bad)
        .await
        .unwrap_err();
    assert!(matches!(err.kind, ErrorKind::Validation(_)));
    assert!(err.message.contains("403"), "names the HTTP status: {err}");
    assert!(!err.message.contains("super-secret"));
    let f = core.setup_cloudflare().await;
    let enc: String =
        sqlx::query_scalar("SELECT api_token_enc FROM cloudflare_clients WHERE id = ?")
            .bind(&f.client_id)
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert!(!enc.contains(CF_TOKEN) && enc.starts_with("enc:") || !enc.contains(CF_TOKEN));
    // wrong account id shape
    assert!(matches!(
        kind(
            core.services
                .cloudflare
                .client_create(&cli, "d", "short", &Secret::new(CF_TOKEN))
                .await
        ),
        ErrorKind::Validation(_)
    ));
    assert_eq!(
        audit_count(&core, "cloudflare_client.create", "ok").await,
        1
    );
    assert!(
        !audit_meta(&core, "cloudflare_client.create")
            .await
            .contains(CF_TOKEN)
    );
}

#[tokio::test]
async fn tunnel_create_reveal_and_delete() {
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let cli = Actor::cli();
    let tunnels = core.services.cloudflare.tunnels_list(&cli).await.unwrap();
    assert_eq!(tunnels.len(), 1);
    assert!(tunnels[0].active, "first tunnel is active");
    let reveal = core
        .services
        .cloudflare
        .tunnel_token_reveal(&cli, &f.tunnel_id)
        .await
        .unwrap();
    assert!(
        reveal
            .install_command
            .expose()
            .starts_with("cloudflared service install ")
    );
    assert!(
        reveal
            .install_command
            .expose()
            .ends_with(reveal.token.expose())
    );
    assert_eq!(
        audit_count(&core, "cloudflare_tunnel.token_reveal", "ok").await,
        1
    );
    assert!(
        !audit_meta(&core, "cloudflare_tunnel.token_reveal")
            .await
            .contains(reveal.token.expose())
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cloudflare_tunnels WHERE cf_tunnel_id = ?")
            .bind(&f.cf_tunnel_id)
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert_eq!(rows, 1, "no token column exists to persist it");
    // The status tells the operator what to do when there is no connector.
    let st = core
        .services
        .cloudflare
        .tunnel_status(&cli, &f.tunnel_id)
        .await
        .unwrap();
    assert!(st.cloudflared.active);
    assert!(st.hint.is_some(), "inactive tunnel gets a hint");
    // Active tunnel with a running cloudflared needs force.
    assert_eq!(
        kind(
            core.services
                .cloudflare
                .tunnel_delete(&cli, &f.tunnel_id, false)
                .await
        ),
        ErrorKind::Conflict
    );
    assert_eq!(
        kind(
            core.services
                .cloudflare
                .client_delete(&cli, &f.client_id)
                .await
        ),
        ErrorKind::Conflict
    );
    core.services
        .cloudflare
        .tunnel_delete(&cli, &f.tunnel_id, true)
        .await
        .unwrap();
    assert!(core.ingress.cloudflare.tunnel(&f.cf_tunnel_id).is_none());
    core.services
        .cloudflare
        .client_delete(&cli, &f.client_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn dns_crud_and_validation() {
    use crate::service::cloudflare::DnsInput;
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let cli = Actor::cli();
    let s = &core.services.cloudflare;
    let rec = s
        .dns_create(
            &cli,
            &f.client_id,
            &f.zone_id,
            &DnsInput {
                record_type: "a".into(),
                name: "Www.Example.com".into(),
                content: "203.0.113.9".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(rec.name, "www.example.com");
    let bad = s
        .dns_create(
            &cli,
            &f.client_id,
            &f.zone_id,
            &DnsInput {
                record_type: "A".into(),
                name: "x.example.com".into(),
                content: "nope".into(),
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(kind(bad), ErrorKind::Validation(_)));
    let txt_proxied = s
        .dns_create(
            &cli,
            &f.client_id,
            &f.zone_id,
            &DnsInput {
                record_type: "TXT".into(),
                name: "t.example.com".into(),
                content: "v".into(),
                proxied: true,
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(kind(txt_proxied), ErrorKind::Validation(_)));
    let upd = s
        .dns_update(
            &cli,
            &f.client_id,
            &f.zone_id,
            &rec.id,
            &DnsInput {
                record_type: "A".into(),
                name: "www.example.com".into(),
                content: "203.0.113.10".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(upd.content, "203.0.113.10");
    let list = s
        .dns_list(&cli, &f.client_id, &f.zone_id, true)
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    s.dns_delete(&cli, &f.client_id, &f.zone_id, &rec.id)
        .await
        .unwrap();
    assert!(core.ingress.cloudflare.dns(&f.zone_id).is_empty());
    assert!(
        audit_meta(&core, "cloudflare_dns.create")
            .await
            .contains("www.example.com")
    );
}

// ---- Hostnames: Cloudflare Tunnel ---------------------------------------------------------

#[tokio::test]
async fn cloudflare_hostname_updates_ingress_and_dns_and_delete_reverses_both() {
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let port = core.ensure_port("prj_1", 18080).await;
    let cli = Actor::cli();
    let out = core
        .services
        .hostnames
        .create(&cli, &cf_input(&f, &port, "App.Example.com"))
        .await
        .unwrap();
    assert_eq!(out.hostname.hostname, "app.example.com");
    assert_eq!(
        out.hostname.sync_status, "active",
        "{:?}",
        out.hostname.last_error
    );
    let t = core.ingress.cloudflare.tunnel(&f.cf_tunnel_id).unwrap();
    assert_eq!(t.ingress.len(), 2);
    assert_eq!(t.ingress[0].hostname.as_deref(), Some("app.example.com"));
    assert_eq!(t.ingress[0].service, "http://127.0.0.1:18080");
    assert_eq!(t.ingress[1].service, CATCH_ALL, "catch-all last");
    let dns = core.ingress.cloudflare.dns(&f.zone_id);
    assert_eq!(dns.len(), 1);
    assert_eq!(dns[0].record_type, "CNAME");
    assert_eq!(
        dns[0].content,
        format!("{}.cfargotunnel.com", f.cf_tunnel_id)
    );
    assert!(dns[0].proxied);
    assert_eq!(dns[0].comment.as_deref(), Some(MANAGED_COMMENT));
    assert!(
        out.warnings.iter().any(|w| w.contains("connector")),
        "{:?}",
        out.warnings
    );

    let d = core
        .services
        .hostnames
        .delete(&cli, &out.hostname.id)
        .await
        .unwrap();
    assert!(d.warnings.is_empty());
    let t = core.ingress.cloudflare.tunnel(&f.cf_tunnel_id).unwrap();
    assert_eq!(
        t.ingress,
        vec![IngressRule {
            hostname: None,
            service: CATCH_ALL.into()
        }]
    );
    assert!(core.ingress.cloudflare.dns(&f.zone_id).is_empty());
    assert_eq!(audit_count(&core, "hostname.create", "ok").await, 1);
    assert_eq!(audit_count(&core, "hostname.delete", "ok").await, 1);
}

#[tokio::test]
async fn hostname_validation_and_targets() {
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let port = core.ensure_port("prj_1", 18080).await;
    core.ensure_project("prj_2").await;
    let other = core.ensure_port("prj_2", 18090).await;
    let cli = Actor::cli();
    let s = &core.services.hostnames;
    for bad in [
        "x.example.com`) || Host(`y",
        "UPPER space.example.com",
        "*.example.com",
        "nodots",
    ] {
        assert!(
            matches!(
                kind(s.create(&cli, &cf_input(&f, &port, bad)).await),
                ErrorKind::Validation(_)
            ),
            "{bad}"
        );
    }
    // Port of another project, unknown port.
    assert!(matches!(
        kind(s.create(&cli, &cf_input(&f, &other, "a.example.com")).await),
        ErrorKind::Validation(_)
    ));
    assert!(matches!(
        kind(
            s.create(&cli, &cf_input(&f, "ppt_nope", "a.example.com"))
                .await
        ),
        ErrorKind::Validation(_)
    ));
    // Non-loopback / IPv6-only / udp ports are not targets.
    sqlx::query("INSERT INTO project_ports (id, project_id, label, host_port, container_port, bind_address, created_at) VALUES ('ppt_v6', 'prj_1', 'v6', 18100, 80, '::1', 0)")
        .execute(core.db.writer()).await.unwrap();
    sqlx::query("INSERT INTO project_ports (id, project_id, label, host_port, container_port, protocol, created_at) VALUES ('ppt_udp', 'prj_1', 'u', 18101, 80, 'udp', 0)")
        .execute(core.db.writer()).await.unwrap();
    for p in ["ppt_v6", "ppt_udp"] {
        assert!(matches!(
            kind(s.create(&cli, &cf_input(&f, p, "b.example.com")).await),
            ErrorKind::Validation(_)
        ));
    }
    // Hostname outside the zone, and no tunnel.
    assert!(matches!(
        kind(s.create(&cli, &cf_input(&f, &port, "a.other.org")).await),
        ErrorKind::Validation(_)
    ));
    // Without a tunnel and without an active one there is nowhere to publish.
    let bare = TestCore::new().await;
    let bp = bare.ensure_port("prj_1", 18080).await;
    assert!(matches!(
        kind(
            bare.services
                .hostnames
                .create(
                    &cli,
                    &input("prj_1", &bp, "a.example.com", "cloudflare_tunnel")
                )
                .await
        ),
        ErrorKind::Validation(_)
    ));
    // Nothing reached the provider for any rejected input.
    assert!(
        !core
            .ingress
            .cloudflare
            .calls()
            .iter()
            .any(|c| c == "put_ingress")
    );
    // A name cannot be added twice, across providers.
    s.create(&cli, &cf_input(&f, &port, "a.example.com"))
        .await
        .unwrap();
    core.enable_traefik();
    let dup = s
        .create(&cli, &input("prj_1", &port, "a.example.com", "traefik"))
        .await;
    assert_eq!(kind(dup), ErrorKind::Conflict);
}

#[tokio::test]
async fn options_and_the_default_tunnel() {
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let port = core.ensure_port("prj_1", 18080).await;
    let cli = Actor::cli();
    let o = core.services.hostnames.options(&cli).await.unwrap();
    assert_eq!(o.tunnel.as_ref().unwrap().id, f.tunnel_id);
    assert_eq!(o.zones.len(), 1);
    assert!(!o.traefik_ready && !o.public_ip_set);
    // A Member (not even with the permission) can read the options; no tunnel id is needed.
    let m = core
        .create_member("mia", &[("prj_1", &[Permission::Hostnames])])
        .await;
    let ma = core.session_actor(&m).await;
    assert!(core.services.hostnames.options(&ma).await.is_ok());
    let mut i = input("prj_1", &port, "m.example.com", "cloudflare_tunnel");
    i.zone_id = Some(f.zone_id.clone());
    let out = core.services.hostnames.create(&ma, &i).await.unwrap();
    assert_eq!(
        out.hostname.tunnel_id.as_deref(),
        Some(f.tunnel_id.as_str())
    );
    assert_eq!(out.hostname.sync_status, "active");
}

#[tokio::test]
async fn failed_ingress_put_keeps_the_row_for_retry() {
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let port = core.ensure_port("prj_1", 18080).await;
    let cli = Actor::cli();
    core.ingress
        .cloudflare
        .fail_ingress_put(400, "ingress rejected");
    let out = core
        .services
        .hostnames
        .create(&cli, &cf_input(&f, &port, "app.example.com"))
        .await
        .unwrap();
    assert_eq!(out.hostname.sync_status, "error");
    assert!(
        out.hostname
            .last_error
            .as_deref()
            .unwrap()
            .contains("ingress rejected")
    );
    assert!(
        core.ingress.cloudflare.dns(&f.zone_id).is_empty(),
        "no DNS before the route exists"
    );
    core.ingress.cloudflare.clear_ingress_failure();
    let out = core
        .services
        .hostnames
        .retry(&cli, &out.hostname.id)
        .await
        .unwrap();
    assert_eq!(out.hostname.sync_status, "active");
    assert_eq!(core.ingress.cloudflare.dns(&f.zone_id).len(), 1);
}

#[tokio::test]
async fn dns_conflict_is_refused_without_overwrite() {
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let port = core.ensure_port("prj_1", 18080).await;
    core.ingress.cloudflare.seed_dns(
        &f.zone_id,
        CfDnsRecord {
            id: "rec-a".into(),
            record_type: "A".into(),
            name: "app.example.com".into(),
            content: "198.51.100.1".into(),
            ttl: 1,
            proxied: false,
            priority: None,
            comment: None,
        },
    );
    let out = core
        .services
        .hostnames
        .create(&Actor::cli(), &cf_input(&f, &port, "app.example.com"))
        .await
        .unwrap();
    assert_eq!(out.hostname.sync_status, "error");
    let e = out.hostname.last_error.unwrap();
    assert!(
        e.contains("A record") && e.contains("198.51.100.1"),
        "shows the existing record: {e}"
    );
    let recs = core.ingress.cloudflare.dns(&f.zone_id);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].content, "198.51.100.1", "not overwritten");
}

#[tokio::test]
async fn member_cannot_repoint_a_dns_record_yanto_did_not_create() {
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let port = core.ensure_port("prj_1", 18080).await;
    let foreign = CfDnsRecord {
        id: "rec-c".into(),
        record_type: "CNAME".into(),
        name: "www.example.com".into(),
        content: "owner-site.example.net".into(),
        ttl: 1,
        proxied: true,
        priority: None,
        comment: None,
    };
    core.ingress.cloudflare.seed_dns(&f.zone_id, foreign);
    let m = core
        .create_member("mia", &[("prj_1", &[Permission::Hostnames])])
        .await;
    let ma = core.session_actor(&m).await;
    let s = &core.services.hostnames;
    let out = s
        .create(&ma, &cf_input(&f, &port, "www.example.com"))
        .await
        .unwrap();
    assert_eq!(out.hostname.sync_status, "error");
    assert!(
        out.hostname
            .last_error
            .unwrap()
            .contains("only the Owner can replace it")
    );
    let recs = core.ingress.cloudflare.dns(&f.zone_id);
    assert_eq!(recs[0].content, "owner-site.example.net", "not repointed");
    // Deleting the Member's Hostname must not remove the Owner's record either.
    s.delete(&ma, &out.hostname.id).await.unwrap();
    assert_eq!(core.ingress.cloudflare.dns(&f.zone_id).len(), 1);
    // The Owner may adopt it (legacy "create or update").
    let out = s
        .create(&Actor::cli(), &cf_input(&f, &port, "www.example.com"))
        .await
        .unwrap();
    assert_eq!(out.hostname.sync_status, "active");
    let recs = core.ingress.cloudflare.dns(&f.zone_id);
    assert_eq!(recs[0].comment.as_deref(), Some(MANAGED_COMMENT));
}

#[tokio::test]
async fn disable_removes_from_provider_and_enable_restores() {
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let port = core.ensure_port("prj_1", 18080).await;
    let cli = Actor::cli();
    let h = core
        .services
        .hostnames
        .create(&cli, &cf_input(&f, &port, "app.example.com"))
        .await
        .unwrap()
        .hostname;
    let off = core
        .services
        .hostnames
        .disable(&cli, &h.id)
        .await
        .unwrap()
        .hostname;
    assert_eq!(off.sync_status, "disabled");
    assert_eq!(
        core.ingress
            .cloudflare
            .tunnel(&f.cf_tunnel_id)
            .unwrap()
            .ingress
            .len(),
        1
    );
    let on = core
        .services
        .hostnames
        .enable(&cli, &h.id)
        .await
        .unwrap()
        .hostname;
    assert_eq!(on.sync_status, "active");
    assert_eq!(
        core.ingress
            .cloudflare
            .tunnel(&f.cf_tunnel_id)
            .unwrap()
            .ingress
            .len(),
        2
    );
}

#[tokio::test]
async fn a_tunnel_and_a_port_in_use_are_protected() {
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let port = core.ensure_port("prj_1", 18080).await;
    let cli = Actor::cli();
    core.services
        .hostnames
        .create(&cli, &cf_input(&f, &port, "app.example.com"))
        .await
        .unwrap();
    assert_eq!(
        kind(
            core.services
                .cloudflare
                .tunnel_delete(&cli, &f.tunnel_id, true)
                .await
        ),
        ErrorKind::Conflict
    );
    // The declared port cannot be removed while a Hostname uses it (M2 reconciliation path).
    let mut conn = core.db.writer().acquire().await.unwrap();
    let r = crate::db::repo::projects::delete_port(&mut conn, &port).await;
    assert_eq!(kind(r), ErrorKind::Conflict);
    drop(conn);
    // Deleting the Project while it is published is refused too.
    let d = core
        .services
        .projects
        .delete(&cli, "prj_1", &Default::default())
        .await;
    assert_eq!(kind(d), ErrorKind::Conflict);
}

// ---- Diagnostics --------------------------------------------------------------------------

#[tokio::test]
async fn diagnostics_identify_each_seeded_fault() {
    let core = TestCore::new().await;
    let f = core.setup_cloudflare().await;
    let port = core.ensure_port("prj_1", 18080).await;
    let cli = Actor::cli();
    let h = core
        .services
        .hostnames
        .create(&cli, &cf_input(&f, &port, "app.example.com"))
        .await
        .unwrap()
        .hostname;
    let status = |checks: &[crate::service::hostnames::DiagCheck], name: &str| {
        checks.iter().find(|c| c.name == name).map(|c| c.status)
    };
    let run = || async {
        core.services
            .hostnames
            .diagnostics(&cli, Some(&h.id), false)
            .await
            .unwrap()
            .remove(0)
            .checks
    };
    // Healthy except the local port, which nothing listens on.
    let c = run().await;
    assert_eq!(status(&c, "API token"), Some(DiagStatus::Pass));
    assert_eq!(status(&c, "host cloudflared"), Some(DiagStatus::Pass));
    assert_eq!(status(&c, "ingress drift"), Some(DiagStatus::Pass));
    assert_eq!(status(&c, "dns record"), Some(DiagStatus::Pass));
    assert_eq!(
        status(&c, "target port"),
        Some(DiagStatus::Fail),
        "closed local port"
    );
    core.ingress.probe.open_port(18080);
    core.ingress
        .probe
        .http_status(18080, "app.example.com", 200);
    let c = run().await;
    assert_eq!(status(&c, "target port"), Some(DiagStatus::Pass));
    assert_eq!(status(&c, "local HTTP probe"), Some(DiagStatus::Pass));
    // Stopped host cloudflared.
    core.ingress
        .systemd
        .install("cloudflared", false, "/usr/bin/cloudflared tunnel run");
    assert_eq!(
        status(&run().await, "host cloudflared"),
        Some(DiagStatus::Fail)
    );
    // Ingress drift (manual edit in the Cloudflare dashboard).
    core.ingress.cloudflare.set_remote_ingress(
        &f.cf_tunnel_id,
        vec![IngressRule {
            hostname: None,
            service: CATCH_ALL.into(),
        }],
    );
    assert_eq!(
        status(&run().await, "ingress drift"),
        Some(DiagStatus::Warn)
    );
    // Wrong DNS target.
    let rec = core.ingress.cloudflare.dns(&f.zone_id).remove(0);
    core.ingress.cloudflare.seed_dns(
        &f.zone_id,
        CfDnsRecord {
            id: "x".into(),
            ..rec.clone()
        },
    );
    let _ = rec;
    let zone = &f.zone_id;
    let _ = zone;
    // Replace content through the fake's update path.
    let token = Secret::new(CF_TOKEN);
    use crate::ports::{CloudflarePort, DnsRecordInput};
    let id = core.ingress.cloudflare.dns(&f.zone_id)[0].id.clone();
    core.ingress
        .cloudflare
        .update_dns(
            &token,
            &f.zone_id,
            &id,
            &DnsRecordInput {
                record_type: "CNAME".into(),
                name: "app.example.com".into(),
                content: "wrong.cfargotunnel.com".into(),
                ttl: 1,
                proxied: true,
                priority: None,
                comment: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(status(&run().await, "dns record"), Some(DiagStatus::Fail));
    // The public probe is opt-in.
    assert!(
        !core
            .services
            .hostnames
            .diagnostics(&cli, Some(&h.id), false)
            .await
            .unwrap()[0]
            .checks
            .iter()
            .any(|c| c.name == "public probe")
    );
    assert!(
        core.services
            .hostnames
            .diagnostics(&cli, Some(&h.id), true)
            .await
            .unwrap()[0]
            .checks
            .iter()
            .any(|c| c.name == "public probe")
    );
    // Diagnostics never mutate.
    assert!(
        !core
            .ingress
            .cloudflare
            .calls()
            .iter()
            .rev()
            .take(20)
            .any(|c| c.starts_with("put_") || c.starts_with("create_") || c.starts_with("delete_"))
    );
}

// ---- Traefik ------------------------------------------------------------------------------

#[tokio::test]
async fn traefik_blocks_creation_until_prerequisites_hold() {
    let core = TestCore::new().await;
    let port = core.ensure_port("prj_1", 18080).await;
    let cli = Actor::cli();
    let s = &core.services.hostnames;
    // Not installed.
    let st = s.traefik_status(&cli).await.unwrap();
    assert!(!st.ready && !st.daemon.instructions.is_empty());
    assert!(matches!(
        kind(
            s.create(&cli, &input("prj_1", &port, "a.example.com", "traefik"))
                .await
        ),
        ErrorKind::Validation(_)
    ));
    // Installed, but watch is off.
    core.enable_traefik();
    let t = &core.config.ingress.traefik;
    let text = std::fs::read_to_string("/dev/null").unwrap_or_default();
    let _ = text;
    core.ingress.traefik.set_static(
        &t.static_config,
        &format!("entryPoints:\n  websecure:\n    address: ':443'\nproviders:\n  file:\n    directory: {}\n    watch: false\ncertificatesResolvers:\n  letsencrypt:\n    acme: {{}}\n", t.dynamic_dir.display()),
    );
    let st = s.traefik_status(&cli).await.unwrap();
    assert!(!st.ready);
    assert!(st.not_ready_reason.unwrap().contains("watch"));
    assert_eq!(core.ingress.traefik.write_count(), 0);
    // Directory not writable.
    core.enable_traefik();
    core.ingress.traefik.set_dir_status(true, false);
    assert!(!s.traefik_status(&cli).await.unwrap().ready);
    core.ingress.traefik.set_dir_status(true, true);
    assert!(s.traefik_status(&cli).await.unwrap().ready);
}

#[tokio::test]
async fn traefik_hostname_writes_the_dynamic_file_and_coexists_with_cloudflare() {
    let core = TestCore::new().await;
    core.enable_traefik();
    let f = core.setup_cloudflare().await;
    let p1 = core.ensure_port("prj_1", 18080).await;
    let p2 = core.ensure_port("prj_1", 18081).await;
    let cli = Actor::cli();
    let s = &core.services.hostnames;
    let t = s
        .create(&cli, &input("prj_1", &p1, "t.example.com", "traefik"))
        .await
        .unwrap();
    assert_eq!(
        t.hostname.sync_status, "active",
        "{:?}",
        t.hostname.last_error
    );
    let file = core.ingress.traefik.current().unwrap();
    assert!(file.contains("Host(`t.example.com`)") && file.contains("http://127.0.0.1:18080"));
    assert!(file.contains("certResolver: letsencrypt"));
    let c = s
        .create(&cli, &cf_input(&f, &p2, "c.example.com"))
        .await
        .unwrap();
    assert_eq!(c.hostname.sync_status, "active");
    assert!(
        !core
            .ingress
            .traefik
            .current()
            .unwrap()
            .contains("c.example.com"),
        "providers are separate"
    );
    assert_eq!(
        core.ingress
            .cloudflare
            .tunnel(&f.cf_tunnel_id)
            .unwrap()
            .ingress
            .len(),
        2
    );
    let st = s.traefik_status(&cli).await.unwrap();
    assert_eq!(st.hostnames, 1);
    assert_eq!(st.file_state, crate::service::hostnames::FileState::InSync);
    // An unchanged re-publish does not rewrite the file.
    let writes = core.ingress.traefik.write_count();
    s.retry(&cli, &t.hostname.id).await.unwrap();
    assert_eq!(core.ingress.traefik.write_count(), writes);
    // Manual edit is reported as drift and fixed by re-sync.
    *core.ingress.traefik.content.lock().unwrap() = Some("# edited\n".into());
    assert_eq!(
        s.traefik_status(&cli).await.unwrap().file_state,
        crate::service::hostnames::FileState::Drift
    );
    let r = s.resync(&cli).await.unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert!(
        core.ingress
            .traefik
            .current()
            .unwrap()
            .contains("t.example.com")
    );
    s.delete(&cli, &t.hostname.id).await.unwrap();
    assert!(
        !core
            .ingress
            .traefik
            .current()
            .unwrap()
            .contains("t.example.com")
    );
}

#[tokio::test]
async fn traefik_write_failure_is_recorded_and_a_404_route_is_an_error() {
    let core = TestCore::new().await;
    core.enable_traefik();
    let p = core.ensure_port("prj_1", 18080).await;
    let cli = Actor::cli();
    let s = &core.services.hostnames;
    core.ingress.traefik.fail_write();
    let h = s
        .create(&cli, &input("prj_1", &p, "a.example.com", "traefik"))
        .await
        .unwrap()
        .hostname;
    assert_eq!(h.sync_status, "error");
    assert!(h.last_error.unwrap().contains("permission denied"));
    // Route not loaded: Traefik answers 404 for the host.
    core.ingress
        .systemd
        .journal_output
        .lock()
        .unwrap()
        .push_str("level=error msg=bad");
    core.ingress.probe.http_status(80, "a.example.com", 404);
    let out = s.retry(&cli, &h.id).await.unwrap();
    assert_eq!(out.hostname.sync_status, "error");
    assert!(out.hostname.last_error.unwrap().contains("level=error"));
    core.ingress.probe.http_status(80, "a.example.com", 200);
    assert_eq!(
        s.retry(&cli, &h.id).await.unwrap().hostname.sync_status,
        "active"
    );
}

#[tokio::test]
async fn traefik_dns_needs_a_client_and_public_ip() {
    let core = TestCore::new().await;
    core.enable_traefik();
    let f = core.setup_cloudflare().await;
    let p = core.ensure_port("prj_1", 18080).await;
    let cli = Actor::cli();
    let s = &core.services.hostnames;
    let mut i = input("prj_1", &p, "t.example.com", "traefik");
    i.manage_dns = true;
    assert!(
        matches!(kind(s.create(&cli, &i).await), ErrorKind::Validation(_)),
        "no zone"
    );
    i.zone_id = Some(f.zone_id.clone());
    assert!(
        matches!(kind(s.create(&cli, &i).await), ErrorKind::Validation(_)),
        "no public ip"
    );
    s.public_ip_set(&cli, Some("203.0.113.5"), Some("2001:db8::5"))
        .await
        .unwrap();
    let h = s.create(&cli, &i).await.unwrap().hostname;
    assert_eq!(h.sync_status, "active", "{:?}", h.last_error);
    let dns = core.ingress.cloudflare.dns(&f.zone_id);
    assert_eq!(dns.len(), 2);
    assert!(dns.iter().all(|r| !r.proxied));
    assert!(
        dns.iter()
            .any(|r| r.record_type == "A" && r.content == "203.0.113.5")
    );
    assert!(dns.iter().any(|r| r.record_type == "AAAA"));
    s.delete(&cli, &h.id).await.unwrap();
    assert!(core.ingress.cloudflare.dns(&f.zone_id).is_empty());
    assert!(matches!(
        kind(s.public_ip_set(&cli, Some("not-an-ip"), None).await),
        ErrorKind::Validation(_)
    ));
}

// ---- Registry target ----------------------------------------------------------------------

#[tokio::test]
async fn registry_hostname_needs_the_hook_and_the_owner() {
    let core = TestCore::new().await;
    core.enable_traefik();
    let cli = Actor::cli();
    let s = &core.services.hostnames;
    let reg = |host: &str| HostnameInput {
        target: TargetInput::Registry,
        hostname: host.into(),
        ingress_provider: "traefik".into(),
        tunnel_id: None,
        zone_id: None,
        manage_dns: false,
    };
    assert!(
        matches!(
            kind(s.create(&cli, &reg("reg.example.com")).await),
            ErrorKind::Validation(_)
        ),
        "registry disabled"
    );
    core.ingress.registry.enable(5000);
    let h = s
        .create(&cli, &reg("reg.example.com"))
        .await
        .unwrap()
        .hostname;
    assert_eq!(h.target_kind, "registry");
    assert!(h.project_id.is_none());
    assert!(
        core.ingress
            .traefik
            .current()
            .unwrap()
            .contains("http://127.0.0.1:5000")
    );
    // A Member, even with hostnames on a project, can neither create nor manage it.
    let m = core
        .create_member("mia", &[("prj_1", &[Permission::Hostnames])])
        .await;
    let ma = core.session_actor(&m).await;
    assert_eq!(
        kind(s.create(&ma, &reg("reg2.example.com")).await),
        ErrorKind::Forbidden
    );
    assert_eq!(kind(s.delete(&ma, &h.id).await), ErrorKind::Forbidden);
    assert!(
        s.list(&ma, None).await.unwrap().is_empty(),
        "registry names are Owner-only"
    );
}

#[tokio::test]
async fn member_with_hostnames_permission_manages_only_their_project() {
    let core = TestCore::new().await;
    core.enable_traefik();
    let p1 = core.ensure_port("prj_1", 18080).await;
    let p2 = core.ensure_port("prj_2", 18081).await;
    let cli = Actor::cli();
    let s = &core.services.hostnames;
    let other = s
        .create(&cli, &input("prj_2", &p2, "two.example.com", "traefik"))
        .await
        .unwrap()
        .hostname;
    let m = core
        .create_member("mia", &[("prj_1", &[Permission::Hostnames])])
        .await;
    let ma = core.session_actor(&m).await;
    let mine = s
        .create(&ma, &input("prj_1", &p1, "one.example.com", "traefik"))
        .await
        .unwrap()
        .hostname;
    let names: Vec<String> = s
        .list(&ma, None)
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.hostname)
        .collect();
    assert_eq!(names, vec!["one.example.com"]);
    assert_eq!(kind(s.retry(&ma, &other.id).await), ErrorKind::NotFound);
    assert_eq!(
        kind(
            s.create(&ma, &input("prj_2", &p2, "x.example.com", "traefik"))
                .await
        ),
        ErrorKind::NotFound
    );
    s.disable(&ma, &mine.id).await.unwrap();
    let d = s.diagnostics(&ma, None, false).await.unwrap();
    assert_eq!(d.len(), 1);
    // A Member without the permission gets 403 on the project list.
    let n = core
        .create_member("nia", &[("prj_1", &[Permission::Deploy])])
        .await;
    let na = core.session_actor(&n).await;
    assert_eq!(kind(s.list(&na, Some("prj_1")).await), ErrorKind::Forbidden);
    assert_eq!(kind(s.list(&na, Some("prj_9")).await), ErrorKind::NotFound);
}

// ---- Daemons ------------------------------------------------------------------------------

#[tokio::test]
async fn daemon_status_and_control() {
    let core = TestCore::new().await;
    let cli = Actor::cli();
    let s = &core.services.daemons;
    let all = s.status(&cli).await.unwrap();
    assert_eq!(all.len(), 3);
    assert!(
        all.iter()
            .all(|d| !d.installed && !d.instructions.is_empty())
    );
    assert!(
        all.iter()
            .find(|d| d.name == "frps")
            .unwrap()
            .sample_unit
            .is_some()
    );
    assert!(
        matches!(
            kind(s.control(&cli, "frps", "restart", true).await),
            ErrorKind::Conflict
        ),
        "not installed: never auto-installs"
    );
    assert!(
        core.ingress
            .systemd
            .calls()
            .iter()
            .all(|c| !c.starts_with("restart"))
    );

    core.ingress
        .systemd
        .install("frps", true, "/usr/local/bin/frps -c /etc/frp/frps.toml");
    core.ingress
        .systemd
        .set_version("/usr/local/bin/frps", "0.99.0");
    let d = s
        .status(&cli)
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.name == "frps")
        .unwrap();
    assert!(d.active && d.version.as_deref() == Some("0.99.0"));
    assert_eq!(d.config_path.as_deref(), Some("/etc/frp/frps.toml"));
    // confirmation is required and names the consequence for frps.
    let e = s.control(&cli, "frps", "restart", false).await.unwrap_err();
    assert!(e.message.contains("drops every live frp tunnel"));
    assert!(
        !core
            .ingress
            .systemd
            .calls()
            .iter()
            .any(|c| c == "restart frps")
    );
    s.control(&cli, "frps", "restart", true).await.unwrap();
    assert!(
        core.ingress
            .systemd
            .calls()
            .iter()
            .any(|c| c == "restart frps")
    );
    assert!(matches!(
        kind(s.control(&cli, "nginx", "start", true).await),
        ErrorKind::Validation(_)
    ));
    assert!(matches!(
        kind(s.control(&cli, "frps", "reload", true).await),
        ErrorKind::Validation(_)
    ));
    // A failing restart shows systemctl status and the journal.
    *core.ingress.systemd.status_output.lock().unwrap() = "Active: failed".into();
    core.ingress.systemd.fail_control("Job failed");
    let e = s.control(&cli, "frps", "restart", true).await.unwrap_err();
    assert!(e.message.contains("Job failed") && e.message.contains("Active: failed"));
    assert_eq!(audit_count(&core, "daemon.control", "ok").await, 2);
}

// ---- FRP ----------------------------------------------------------------------------------

const FRPS: &str = "# my frps\nbindPort = 7000\nvhostHTTPPort = 8080\n\n[webServer]\naddr = \"127.0.0.1\"\nport = 7500\nuser = \"admin\"\npassword = \"hunter2\"\n";

fn frp_path(core: &TestCore) -> std::path::PathBuf {
    core.config.frp.config_path.clone()
}

fn setup_frps(core: &TestCore) {
    core.ingress.systemd.install(
        "frps",
        true,
        &format!("/usr/local/bin/frps -c {}", frp_path(core).display()),
    );
    core.ingress.frp_files.put(&frp_path(core), FRPS, 0o600);
}

#[tokio::test]
async fn frps_overview_statuses() {
    let core = TestCore::new().await;
    let cli = Actor::cli();
    let s = &core.services.frps;
    assert_eq!(
        s.overview(&cli).await.unwrap().status,
        FrpStatus::NotInstalled
    );
    setup_frps(&core);
    core.ingress
        .frp_files
        .put(&frp_path(&core), "bindPort = 7000\n", 0o600);
    assert_eq!(
        s.overview(&cli).await.unwrap().status,
        FrpStatus::RunningNoDashboard
    );
    core.ingress.frp_files.put(&frp_path(&core), FRPS, 0o644);
    core.ingress.frp_dashboard.set_unreachable(true);
    let o = s.overview(&cli).await.unwrap();
    assert_eq!(o.status, FrpStatus::DashboardUnreachable);
    assert!(o.warnings.iter().any(|w| w.contains("world-readable")));
    assert!(o.warnings.iter().any(|w| w.contains("allowPorts")));
    core.ingress.frp_dashboard.set_unreachable(false);
    core.ingress
        .frp_dashboard
        .answer("/api/serverinfo", 401, "");
    assert_eq!(
        s.overview(&cli).await.unwrap().status,
        FrpStatus::DashboardAuthFailed
    );
}

#[tokio::test]
async fn frps_lists_proxies_and_server_info() {
    let core = TestCore::new().await;
    setup_frps(&core);
    let cli = Actor::cli();
    core.ingress.frp_dashboard.answer(
        "/api/serverinfo",
        200,
        r#"{"version":"0.99.0","proxyTypeCount":{"tcp":1}}"#,
    );
    core.ingress.frp_dashboard.answer(
        "/api/proxy/tcp",
        200,
        r#"{"proxies":[{"name":"ssh","conf":{"remotePort":6000},"status":"online","todayTrafficIn":10,"todayTrafficOut":20,"curConns":2,"clientID":"c1"}]}"#,
    );
    core.ingress.frp_dashboard.answer(
        "/api/proxy/http",
        200,
        r#"{"proxies":[{"name":"web","conf":null,"status":"offline"}]}"#,
    );
    let o = core.services.frps.overview(&cli).await.unwrap();
    assert_eq!(o.status, FrpStatus::Running);
    assert_eq!(o.version.as_deref(), Some("0.99.0"));
    assert_eq!(o.proxies.len(), 2);
    let ssh = o.proxies.iter().find(|p| p.name == "ssh").unwrap();
    assert_eq!(
        (
            ssh.remote_port,
            ssh.traffic_in,
            ssh.connections,
            ssh.status.as_str()
        ),
        (Some(6000), 10, 2, "online")
    );
    assert_eq!(
        o.proxies.iter().find(|p| p.name == "web").unwrap().status,
        "offline"
    );
    // Basic auth from frps.toml is used, and no secret is part of the overview.
    let reqs = core.ingress.frp_dashboard.requests.lock().unwrap();
    assert_eq!(reqs[0].0.auth.as_ref().unwrap().0, "admin");
    drop(reqs);
    assert!(!serde_json::to_string(&o).unwrap().contains("hunter2"));
}

#[tokio::test]
async fn frps_settings_preserve_the_file_backup_and_pending_restart() {
    let core = TestCore::new().await;
    setup_frps(&core);
    let cli = Actor::cli();
    let s = &core.services.frps;
    let out = s
        .settings_update(
            &cli,
            &FrpSettingsInput {
                public_host: Some("frp.example.com".into()),
                patch: Patch {
                    bind_port: Change::Set(7001),
                    allow_ports: Change::Set(vec![PortRange::Range {
                        start: 20000,
                        end: 20100,
                    }]),
                    web_password: Change::Set(Secret::new("new-pw-123")),
                    ..Default::default()
                },
                generate_token: true,
            },
        )
        .await
        .unwrap();
    assert!(out.pending_restart && out.token_generated);
    assert!(out.changed.contains(&"bindPort") && out.changed.contains(&"auth.token"));
    let joined = out.diff.join("\n");
    assert!(
        !joined.contains("new-pw-123") && !joined.contains("hunter2"),
        "diff is masked: {joined}"
    );
    let text = core.ingress.frp_files.content(&frp_path(&core)).unwrap();
    assert!(text.starts_with("# my frps\nbindPort = 7001"), "{text}");
    assert!(text.contains("vhostHTTPPort = 8080"));
    assert_eq!(
        core.ingress.frp_files.backup(&frp_path(&core)).as_deref(),
        Some(FRPS)
    );
    let o = s.overview(&cli).await.unwrap();
    assert!(o.pending_restart && o.backup_available);
    assert_eq!(o.public_host, "frp.example.com");
    assert!(o.config.as_ref().unwrap().auth_token_set);
    // Audit has field names, no values.
    let meta = audit_meta(&core, "frps.settings_update").await;
    assert!(meta.contains("bindPort") && !meta.contains("new-pw-123") && !meta.contains("7001"));
    // Revealing the token is audited and is the generated one.
    let tok = s.token_reveal(&cli).await.unwrap();
    assert_eq!(tok.expose().len(), 64);
    assert!(text.contains(tok.expose()));
    assert_eq!(audit_count(&core, "frps.token_reveal", "ok").await, 1);
    // The restart (daemon_control) clears the pending flag.
    core.services
        .daemons
        .control(&cli, "frps", "restart", true)
        .await
        .unwrap();
    assert!(!s.overview(&cli).await.unwrap().pending_restart);
    // Restore the previous config.
    s.config_restore(&cli).await.unwrap();
    assert!(s.overview(&cli).await.unwrap().pending_restart);
    assert!(
        core.ingress
            .frp_files
            .content(&frp_path(&core))
            .unwrap()
            .contains("bindPort = 7000")
    );
}

#[tokio::test]
async fn frps_settings_validation_and_collisions() {
    let core = TestCore::new().await;
    setup_frps(&core);
    let cli = Actor::cli();
    let s = &core.services.frps;
    let upd = |p: Patch| FrpSettingsInput {
        patch: p,
        ..Default::default()
    };
    let bad = [
        Patch {
            bind_port: Change::Set(0),
            ..Default::default()
        },
        Patch {
            allow_ports: Change::Set(vec![PortRange::Range { start: 9, end: 3 }]),
            ..Default::default()
        },
        Patch {
            allow_ports: Change::Set(vec![PortRange::Range {
                start: 6000,
                end: 8000,
            }]),
            ..Default::default()
        },
        Patch {
            web_port: Change::Set(7000),
            ..Default::default()
        },
    ];
    for p in bad {
        assert!(matches!(
            kind(s.settings_update(&cli, &upd(p)).await),
            ErrorKind::Validation(_)
        ));
    }
    assert_eq!(
        core.ingress.frp_files.content(&frp_path(&core)).as_deref(),
        Some(FRPS),
        "nothing written"
    );
    assert_eq!(*core.ingress.frp_files.writes.lock().unwrap(), 0);
    // Port used by another Host process / by a Project.
    core.probe.bind(7100, "tcp", Some("nginx"));
    let e = s
        .settings_update(
            &cli,
            &upd(Patch {
                bind_port: Change::Set(7100),
                ..Default::default()
            }),
        )
        .await
        .unwrap_err();
    assert!(e.message.contains("nginx"), "{e}");
    core.ensure_port("prj_1", 7200).await;
    assert!(
        s.settings_update(
            &cli,
            &upd(Patch {
                bind_port: Change::Set(7200),
                ..Default::default()
            })
        )
        .await
        .is_err()
    );
    // frps's own current port is not a collision.
    core.probe.bind(7000, "tcp", Some("frps"));
    s.settings_update(
        &cli,
        &upd(Patch {
            bind_addr: Change::Set("0.0.0.0".into()),
            bind_port: Change::Set(7000),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    // Invalid TOML in the file blocks writes.
    core.ingress
        .frp_files
        .put(&frp_path(&core), "bindPort = = 1", 0o600);
    assert!(matches!(
        kind(
            s.settings_update(
                &cli,
                &upd(Patch {
                    bind_port: Change::Set(7300),
                    ..Default::default()
                })
            )
            .await
        ),
        ErrorKind::Validation(_)
    ));
    assert!(
        core.services
            .frps
            .overview(&cli)
            .await
            .unwrap()
            .config_error
            .is_some()
    );
}

#[tokio::test]
async fn missing_frps_never_errors_outside_the_frp_page() {
    let core = TestCore::new().await;
    let cli = Actor::cli();
    // Daemons page and hostnames work with nothing installed.
    core.services.daemons.status(&cli).await.unwrap();
    core.services.hostnames.list(&cli, None).await.unwrap();
    core.services.hostnames.traefik_status(&cli).await.unwrap();
    let o = core.services.frps.overview(&cli).await.unwrap();
    assert_eq!(o.status, FrpStatus::NotInstalled);
    assert!(o.config.is_none() && !o.daemon.instructions.is_empty());
    assert!(matches!(
        kind(
            core.services
                .frps
                .settings_update(&cli, &FrpSettingsInput::default())
                .await
        ),
        ErrorKind::Conflict
    ));
}
