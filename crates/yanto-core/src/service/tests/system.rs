use std::sync::Arc;

use super::{audit_count, kind};
use crate::ErrorKind;
use crate::ports::{DockerError, PruneKind, PruneOutcome, PrunePreviewRaw};
use crate::service::system::{CONFIRM_CONTAINERS, CleanupSelection, HealthStatus};
use crate::testing::docker::FakeDocker;
use crate::testing::{Reply, TestCore};

async fn owner(core: &TestCore) -> crate::actor::Actor {
    let o = core.setup_owner().await;
    core.session_actor(&o).await
}

fn preview() -> PrunePreviewRaw {
    PrunePreviewRaw {
        build_cache: vec![FakeDocker::candidate("bc1", "layer", Some(1000))],
        images: vec![FakeDocker::candidate("sha256:aa", "sha256:aa", Some(500))],
        networks: vec![FakeDocker::candidate("n1", "old_default", None)],
        containers: vec![{
            let mut c = FakeDocker::candidate("c1", "old-job", None);
            c.labels
                .insert("com.docker.compose.project".into(), "shop".into());
            c
        }],
    }
}

#[tokio::test]
async fn usage_renders_host_stats_even_when_docker_is_down() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    let ok = core.services.system.usage(&o).await.unwrap();
    assert_eq!(ok.host.cpu_count, 4);
    assert!(ok.docker.info.is_some() && ok.docker.error.is_none());
    // Disk paths asked for: root, data dir, projects dir (+ Docker root).
    let asked = core.host_stats.asked_paths();
    assert!(asked.iter().any(|p| p == std::path::Path::new("/")));
    assert!(asked.iter().any(|p| p == &core.config.paths.data_dir));

    core.docker
        .fail_next(DockerError::Unavailable("down".into()));
    let down = core.services.system.usage(&o).await.unwrap();
    assert!(down.docker.info.is_none() && down.docker.error.is_some());
    assert_eq!(down.host.cpu_count, 4);
}

#[tokio::test]
async fn preview_lists_four_groups_never_volumes_and_defaults_exclude_containers() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    core.docker.set_preview(preview());
    let p = core.services.system.cleanup_preview(&o).await.unwrap();
    let kinds: Vec<_> = p.groups.iter().map(|g| g.kind).collect();
    assert_eq!(kinds, PruneKind::ORDER);
    assert!(!serde_json::to_string(&p).unwrap().contains("volume"));
    let by = |k| p.groups.iter().find(|g| g.kind == k).unwrap();
    assert!(by(PruneKind::BuildCache).default_selected);
    assert!(by(PruneKind::Images).default_selected);
    assert!(by(PruneKind::Networks).default_selected);
    assert!(!by(PruneKind::Containers).default_selected);
    assert!(by(PruneKind::Containers).needs_confirm);
    assert_eq!(by(PruneKind::BuildCache).total_size, 1000);
    // No prune call happened for a preview.
    assert!(core.docker.calls().iter().all(|c| !c.starts_with("prune ")));
}

#[tokio::test]
async fn defaults_leave_stopped_containers_alone_and_audit_totals() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    core.docker.set_prune_outcome(PruneOutcome {
        deleted: 2,
        reclaimed_bytes: 700,
    });
    let r = core
        .services
        .system
        .cleanup(&o, &CleanupSelection::defaults())
        .await
        .unwrap();
    assert_eq!(r.steps.len(), 3);
    assert_eq!(r.reclaimed_bytes, 2100);
    let prunes: Vec<String> = core
        .docker
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("prune "))
        .collect();
    assert_eq!(
        prunes,
        ["prune build_cache", "prune images", "prune networks"]
    );
    assert_eq!(audit_count(&core, "system.cleanup", "ok").await, 1);
    let meta: String =
        sqlx::query_scalar("SELECT metadata FROM audit_logs WHERE action = 'system.cleanup'")
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    assert!(meta.contains("\"reclaimed_bytes\":2100"), "{meta}");
}

#[tokio::test]
async fn stopped_containers_need_the_typed_confirmation_and_run_last() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    let mut sel = CleanupSelection::defaults();
    sel.containers = true;
    let err = core.services.system.cleanup(&o, &sel).await.unwrap_err();
    assert!(matches!(err.kind, ErrorKind::Validation(_)));
    sel.confirm = Some("yes".into());
    assert!(core.services.system.cleanup(&o, &sel).await.is_err());
    assert!(core.docker.calls().iter().all(|c| !c.starts_with("prune ")));

    sel.confirm = Some(CONFIRM_CONTAINERS.into());
    core.services.system.cleanup(&o, &sel).await.unwrap();
    let prunes: Vec<String> = core
        .docker
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("prune "))
        .collect();
    assert_eq!(prunes.last().unwrap(), "prune containers");
    assert_eq!(prunes.len(), 4);

    let none = CleanupSelection::default();
    assert!(core.services.system.cleanup(&o, &none).await.is_err());
}

#[tokio::test]
async fn a_failing_step_is_reported_and_later_steps_still_run() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    core.docker.fail_next(DockerError::Other("boom".into()));
    let r = core
        .services
        .system
        .cleanup(&o, &CleanupSelection::defaults())
        .await
        .unwrap();
    assert!(!r.steps[0].ok && r.steps[0].error.is_some());
    assert!(r.steps[1].ok && r.steps[2].ok);
}

#[tokio::test]
async fn health_reports_each_dependency_independently() {
    let core = TestCore::new().await;
    let o = owner(&core).await;
    let ok = core.services.system.health(&o).await.unwrap();
    assert_eq!(ok.status, HealthStatus::Ok);
    assert_eq!(ok.checks.len(), 4);
    let compose = ok.checks.iter().find(|c| c.name == "compose").unwrap();
    assert!(compose.detail.starts_with("docker compose "), "{compose:?}");

    core.docker
        .fail_next(DockerError::Unavailable("socket gone".into()));
    let degraded = core.services.system.health(&o).await.unwrap();
    assert_eq!(degraded.status, HealthStatus::Degraded);
    let by = |n: &str| degraded.checks.iter().find(|c| c.name == n).unwrap();
    assert_eq!(by("database").status, HealthStatus::Ok);
    assert_eq!(by("docker").status, HealthStatus::Degraded);
}

#[tokio::test]
async fn system_operations_are_owner_only() {
    let core = TestCore::new().await;
    core.setup_owner().await;
    let m = core.create_member("m", &[]).await;
    let m = core.session_actor(&m).await;
    assert_eq!(
        kind(core.services.system.usage(&m).await),
        ErrorKind::Forbidden
    );
    assert_eq!(
        kind(core.services.system.cleanup_preview(&m).await),
        ErrorKind::Forbidden
    );
    assert_eq!(
        kind(
            core.services
                .system
                .cleanup(&m, &CleanupSelection::defaults())
                .await
        ),
        ErrorKind::Forbidden
    );
    // Health is readable by any authenticated actor.
    core.services.system.health(&m).await.unwrap();
}

#[tokio::test]
async fn missing_compose_plugin_degrades_and_extra_checks_can_be_registered() {
    use crate::service::system::{HealthCheck, HealthContributor};
    struct Daemon(HealthStatus);
    #[async_trait::async_trait]
    impl HealthContributor for Daemon {
        async fn checks(&self) -> Vec<HealthCheck> {
            vec![HealthCheck {
                name: "frps",
                status: self.0,
                detail: "scripted".into(),
            }]
        }
    }
    let core = TestCore::new().await;
    let o = owner(&core).await;
    core.commands
        .on("docker compose version", Reply::fail(1, "unknown command"));
    let r = core.services.system.health(&o).await.unwrap();
    let compose = r.checks.iter().find(|c| c.name == "compose").unwrap();
    assert_eq!(
        (compose.status, compose.detail.as_str()),
        (HealthStatus::Degraded, "docker compose: missing")
    );
    assert_eq!(r.status, HealthStatus::Degraded);

    // A hook from another area adds its own line without touching the built-in probes.
    core.commands
        .on("docker compose version", Reply::stdout("v2.40.0\n"));
    core.services
        .system
        .register_health(Arc::new(Daemon(HealthStatus::Ok)));
    let r = core.services.system.health(&o).await.unwrap();
    assert_eq!(r.status, HealthStatus::Ok);
    assert!(r.checks.iter().any(|c| c.name == "frps"));
    assert_eq!(
        r.checks
            .iter()
            .find(|c| c.name == "compose")
            .unwrap()
            .detail,
        "docker compose v2.40.0"
    );
}

#[tokio::test]
async fn app_logs_are_owner_only_capped_and_followable() {
    use crate::applog::LogBuffer;
    let core = TestCore::new().await;
    let o = owner(&core).await;
    let buf = Arc::new(LogBuffer::new(500));
    let sys = core.services.system.clone().with_log_buffer(buf.clone());
    for i in 0..600 {
        buf.push(i, "INFO", "yanto", &format!("line {i} token=abc"));
    }
    let all = sys.logs(&o, None).await.unwrap();
    assert_eq!(all.len(), 500);
    assert_eq!(all[0].text, "line 100 token=[redacted]");
    assert_eq!(sys.logs(&o, Some(2)).await.unwrap().len(), 2);
    let (backlog, mut rx) = sys.logs_stream(&o, Some(1)).await.unwrap();
    assert_eq!(backlog[0].text, "line 599 token=[redacted]");
    buf.push(1000, "WARN", "yanto", "live");
    assert_eq!(rx.recv().await.unwrap().text, "live");

    let m = core.create_member("m", &[]).await;
    let m = core.session_actor(&m).await;
    assert_eq!(kind(sys.logs(&m, None).await), ErrorKind::Forbidden);
    assert!(matches!(
        sys.logs_stream(&m, None).await,
        Err(e) if e.kind == ErrorKind::Forbidden
    ));
}
