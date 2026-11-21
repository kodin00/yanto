//! Runtime settings: validation, persistence, audit, startup overlay.

use serde_json::json;

use crate::ErrorKind;
use crate::actor::Actor;
use crate::config::Config;
use crate::service::settings::{SettingsPatch, apply_overrides};
use crate::testing::TestCore;

fn patch(items: &[(&str, serde_json::Value)]) -> SettingsPatch {
    items
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

#[tokio::test]
async fn read_shows_defaults_and_host_values() {
    let core = TestCore::new().await;
    let v = core.services.settings.read(&Actor::cli()).await.unwrap();
    let c = v
        .runtime
        .iter()
        .find(|e| e.key == "deploy.max_concurrent")
        .unwrap();
    assert_eq!(c.value, json!(2));
    assert!(!c.overridden);
    assert!(v.host.iter().any(|h| h.label == "Authorized Keys user"));
}

#[tokio::test]
async fn update_validates_persists_audits_and_resets() {
    let core = TestCore::new().await;
    let a = Actor::cli();
    let s = &core.services.settings;
    for bad in [
        patch(&[("deploy.max_concurrent", json!(0))]),
        patch(&[("deploy.max_concurrent", json!("3"))]),
        patch(&[("runtime.command_timeout_ms", json!(10))]),
        patch(&[("runtime.trusted_proxies", json!(["not-an-ip"]))]),
        patch(&[("runtime.trusted_proxies", json!(["10.0.0.0/33"]))]),
        patch(&[("nope", json!(1))]),
        patch(&[]),
    ] {
        let e = s.update(&a, &bad).await.unwrap_err();
        assert!(matches!(e.kind, ErrorKind::Validation(_)), "{bad:?}");
    }
    // One bad entry rejects the whole patch.
    let e = s
        .update(
            &a,
            &patch(&[("deploy.max_concurrent", json!(4)), ("nope", json!(1))]),
        )
        .await;
    assert!(e.is_err());
    assert!(!s.read(&a).await.unwrap().runtime[0].overridden);

    let v = s
        .update(
            &a,
            &patch(&[
                ("deploy.max_concurrent", json!(4)),
                ("runtime.trusted_proxies", json!([" 10.0.0.0/8 ", "::1"])),
            ]),
        )
        .await
        .unwrap();
    let get = |k: &str| v.runtime.iter().find(|e| e.key == k).unwrap().clone();
    assert_eq!(get("deploy.max_concurrent").value, json!(4));
    assert!(get("deploy.max_concurrent").overridden);
    assert_eq!(
        get("runtime.trusted_proxies").value,
        json!(["10.0.0.0/8", "::1"])
    );
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE action = 'settings.update'")
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert_eq!(n, 1);

    let v = s
        .update(&a, &patch(&[("deploy.max_concurrent", json!(null))]))
        .await
        .unwrap();
    assert_eq!(get_val(&v, "deploy.max_concurrent"), json!(2));
}

fn get_val(v: &crate::service::settings::SettingsView, k: &str) -> serde_json::Value {
    v.runtime.iter().find(|e| e.key == k).unwrap().value.clone()
}

#[tokio::test]
async fn stored_values_overlay_the_file_config_at_startup() {
    let core = TestCore::new().await;
    core.services
        .settings
        .update(
            &Actor::cli(),
            &patch(&[
                ("deploy.max_concurrent", json!(5)),
                ("deploy.retention_per_project", json!(7)),
                ("runtime.command_timeout_ms", json!(120_000)),
                ("runtime.deployment_log_max_bytes", json!(20_000)),
            ]),
        )
        .await
        .unwrap();
    let mut cfg = Config::default();
    apply_overrides(&core.db, &mut cfg).await;
    assert_eq!(cfg.deploy.max_concurrent, 5);
    assert_eq!(cfg.deploy.retention_per_project, 7);
    assert_eq!(cfg.deploy.command_timeout, "120s");
    assert_eq!(cfg.deploy.log_max_bytes, 20_000);
}
