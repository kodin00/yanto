use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::{Actor, Permission, Scope};
use crate::error::CoreError;
use crate::ports::{ContainerInfo, ContainerStats, DockerError, LogLine, LogStream};
use crate::service::containers::{ContainerProjects, ContainerService, EXEC_PER_ACCOUNT};
use crate::testing::TestCore;

/// Models Projects: the Compose project `shop` belongs to Project `prj_1`.
struct ShopIsPrj1;

#[async_trait::async_trait]
impl ContainerProjects for ShopIsPrj1 {
    async fn project_for(&self, c: &ContainerInfo) -> Result<Option<String>, CoreError> {
        Ok((c.compose_project() == Some("shop")).then(|| "prj_1".to_owned()))
    }
}

struct World {
    core: TestCore,
    svc: ContainerService,
    owner: Actor,
    shop: String,
    stray: String,
}

async fn world() -> World {
    let core = TestCore::new().await;
    let owner = core.setup_owner().await;
    let owner = core.session_actor(&owner).await;
    let shop = core.docker.add_container(
        "shop-web-1",
        "running",
        &[("com.docker.compose.project", "shop")],
    );
    let stray = core.docker.add_container("stray", "running", &[]);
    let svc = core
        .services
        .containers
        .clone()
        .with_projects(Arc::new(ShopIsPrj1))
        .with_linger_check(Duration::from_millis(20));
    World {
        core,
        svc,
        owner,
        shop,
        stray,
    }
}

async fn member(w: &World, name: &str, perms: &[Permission]) -> Actor {
    let u = w.core.create_member(name, &[("prj_1", perms)]).await;
    w.core.session_actor(&u).await
}

#[tokio::test]
async fn owner_sees_everything_members_only_their_projects() {
    let w = world().await;
    let all = w.svc.list(&w.owner).await.unwrap();
    assert_eq!(all.containers.len(), 2);
    let shop = all
        .containers
        .iter()
        .find(|c| c.name == "shop-web-1")
        .unwrap();
    assert_eq!(shop.project.as_deref(), Some("prj_1"));
    assert_eq!(shop.compose_project.as_deref(), Some("shop"));

    let alice = member(&w, "alice", &[]).await;
    let mine = w.svc.list(&alice).await.unwrap();
    assert_eq!(mine.containers.len(), 1);
    assert_eq!(mine.containers[0].name, "shop-web-1");
}

#[tokio::test]
async fn without_a_project_mapping_members_see_nothing_and_owner_everything() {
    let w = world().await;
    let alice = member(&w, "alice", &[Permission::Runtime]).await;
    // The default service (NoProjects): every container is Owner-only.
    let plain = &w.core.services.containers;
    assert!(plain.list(&alice).await.unwrap().containers.is_empty());
    assert_eq!(plain.list(&w.owner).await.unwrap().containers.len(), 2);
    assert_eq!(
        kind(plain.start(&alice, &w.shop).await),
        ErrorKind::NotFound
    );
    plain.restart(&w.owner, &w.shop).await.unwrap();
}

#[tokio::test]
async fn actions_need_runtime_on_the_owning_project() {
    let w = world().await;
    let viewer = member(&w, "viewer", &[]).await;
    let runner = member(&w, "runner", &[Permission::Runtime]).await;

    // View-only: can read but not act.
    w.svc.get(&viewer, &w.shop).await.unwrap();
    assert_eq!(
        kind(w.svc.stop(&viewer, &w.shop).await),
        ErrorKind::Forbidden
    );
    assert_eq!(
        kind(w.svc.start(&viewer, &w.shop).await),
        ErrorKind::Forbidden
    );
    assert_eq!(
        kind(w.svc.restart(&viewer, &w.shop).await),
        ErrorKind::Forbidden
    );
    assert!(w.core.docker.calls().iter().all(|c| !c.starts_with("stop")));

    w.svc.stop(&runner, &w.shop).await.unwrap();
    w.svc.start(&runner, &w.shop).await.unwrap();
    w.svc.restart(&runner, &w.shop).await.unwrap();
    assert_eq!(audit_count(&w.core, "container.stop", "ok").await, 1);
    assert_eq!(audit_count(&w.core, "container.start", "ok").await, 1);
    assert_eq!(audit_count(&w.core, "container.restart", "ok").await, 1);
    let project: String =
        sqlx::query_scalar("SELECT project_id FROM audit_logs WHERE action = 'container.stop'")
            .fetch_one(w.core.db.reader())
            .await
            .unwrap();
    assert_eq!(project, "prj_1");

    // A container outside any Project is invisible to Members (404), even with runtime elsewhere.
    assert_eq!(
        kind(w.svc.stop(&runner, &w.stray).await),
        ErrorKind::NotFound
    );
    assert_eq!(
        kind(w.svc.get(&runner, &w.stray).await),
        ErrorKind::NotFound
    );
    assert_eq!(
        kind(w.svc.logs(&runner, &w.stray, None).await),
        ErrorKind::NotFound
    );
}

#[tokio::test]
async fn token_scope_applies_on_top_of_the_project_rule() {
    let w = world().await;
    let u = w
        .core
        .create_member("tok", &[("prj_1", &[Permission::Runtime])])
        .await;
    let (_, read) = w.core.token_actor(&u, Scope::Read).await;
    let (_, write) = w.core.token_actor(&u, Scope::Write).await;
    w.svc.get(&read, &w.shop).await.unwrap();
    assert_eq!(
        kind(w.svc.start(&read, &w.shop).await),
        ErrorKind::Forbidden
    );
    w.svc.start(&write, &w.shop).await.unwrap();
    assert_eq!(
        kind(w.svc.stop(&write, &w.shop).await),
        ErrorKind::Forbidden,
        "stop needs admin scope"
    );
}

#[tokio::test]
async fn registry_container_is_protected_and_bad_ids_are_not_found() {
    let w = world().await;
    let reg = w.core.docker.add_container(
        "yanto-registry",
        "running",
        &[("yanto.managed", "registry")],
    );
    assert_eq!(kind(w.svc.stop(&w.owner, &reg).await), ErrorKind::Conflict);
    assert_eq!(
        kind(w.svc.restart(&w.owner, &reg).await),
        ErrorKind::Conflict
    );
    for bad in ["", "../etc", "a/b", "x y", "-rf"] {
        assert_eq!(
            kind(w.svc.get(&w.owner, bad).await),
            ErrorKind::NotFound,
            "{bad:?}"
        );
    }
    assert_eq!(kind(w.svc.get(&w.owner, "nope").await), ErrorKind::NotFound);
}

#[tokio::test]
async fn docker_down_maps_to_unavailable() {
    let w = world().await;
    w.core
        .docker
        .fail_next(DockerError::Unavailable("socket".into()));
    assert_eq!(kind(w.svc.list(&w.owner).await), ErrorKind::Unavailable);
}

#[tokio::test]
async fn list_is_cached_for_five_seconds_and_invalidated_by_actions() {
    let w = world().await;
    w.core.docker.set_stats(
        &w.shop,
        ContainerStats {
            cpu_percent: 12.0,
            mem_usage: 100,
            mem_limit: 200,
        },
    );
    let first = w.svc.list(&w.owner).await.unwrap();
    let web = first
        .containers
        .iter()
        .find(|c| c.name == "shop-web-1")
        .unwrap();
    assert_eq!(web.cpu_percent, Some(12.0));
    w.svc.list(&w.owner).await.unwrap();
    assert_eq!(
        w.core.docker.list_calls(),
        1,
        "second read is served from cache"
    );

    w.core.clock.advance(Duration::from_secs(6));
    w.svc.list(&w.owner).await.unwrap();
    assert_eq!(w.core.docker.list_calls(), 2, "cache expires after 5 s");

    w.svc.restart(&w.owner, &w.shop).await.unwrap();
    w.svc.list(&w.owner).await.unwrap();
    assert_eq!(
        w.core.docker.list_calls(),
        3,
        "an action bumps the revision"
    );
}

#[tokio::test]
async fn logs_default_to_500_and_stream_follows() {
    let w = world().await;
    let lines: Vec<LogLine> = (0..700)
        .map(|i| LogLine {
            stream: LogStream::Stdout,
            timestamp: None,
            text: format!("line {i}"),
        })
        .collect();
    w.core.docker.set_logs(lines);
    let out = w.svc.logs(&w.owner, &w.shop, None).await.unwrap();
    assert_eq!(out.len(), 500);
    assert_eq!(out.last().unwrap().text, "line 699");
    assert!(w.core.docker.calls().iter().any(|c| c.contains("tail=500")));

    w.core.docker.set_follow_logs(vec![LogLine {
        stream: LogStream::Stderr,
        timestamp: Some("2026-01-01T00:00:00Z".into()),
        text: "live".into(),
    }]);
    let mut s = w.svc.logs_stream(&w.owner, &w.shop, None).await.unwrap();
    let first = futures_util::StreamExt::next(&mut s)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.text, "live");
    assert_eq!(first.stream, LogStream::Stderr);
    // `since` defaults to the (fake) clock: 2026-01-01.
    assert!(
        w.core
            .docker
            .calls()
            .iter()
            .any(|c| c.ends_with("since=1767225600"))
    );
}

#[tokio::test]
async fn exec_session_is_audited_resizable_and_hangs_up_on_finish() {
    let w = world().await;
    let (io, ticket) = w.svc.exec_open(&w.owner, &w.shop, 100, 30).await.unwrap();
    assert_eq!(audit_count(&w.core, "container.exec_start", "ok").await, 1);
    assert_eq!(w.svc.open_exec_sessions(), 1);
    assert!(
        w.core
            .docker
            .calls()
            .iter()
            .any(|c| c.contains("exec_start") && c.ends_with("100x30"))
    );

    let mut input = io.input;
    input.write_all(b"ls\n").await.unwrap();
    ticket.resize(120, 40).await.unwrap();
    assert_eq!(w.core.docker.resizes(), vec![(120, 40)]);

    w.core.clock.advance(Duration::from_secs(3));
    w.core.docker.set_exec_state(false, Some(0));
    ticket.finish(input, 3, 10, "closed").await;
    assert!(
        w.core.docker.exec_input_closed(),
        "input closed = shell hung up"
    );
    assert_eq!(w.core.docker.exec_input(), b"ls\n");
    assert_eq!(w.svc.open_exec_sessions(), 0);
    assert_eq!(audit_count(&w.core, "container.exec_end", "ok").await, 1);
    let meta: String =
        sqlx::query_scalar("SELECT metadata FROM audit_logs WHERE action = 'container.exec_end'")
            .fetch_one(w.core.db.reader())
            .await
            .unwrap();
    let meta: serde_json::Value = serde_json::from_str(&meta).unwrap();
    assert_eq!(meta["bytes_in"], 3);
    assert_eq!(meta["bytes_out"], 10);
    assert_eq!(meta["duration_ms"], 3000);
    assert_eq!(meta["exit_code"], 0);
    // Keystrokes and output are never recorded.
    assert!(!meta.to_string().contains("ls\\n"));
}

#[tokio::test]
async fn lingering_exec_process_is_flagged_in_audit() {
    let w = world().await;
    let (io, ticket) = w.svc.exec_open(&w.owner, &w.shop, 80, 24).await.unwrap();
    w.core.docker.set_exec_state(true, None);
    ticket.finish(io.input, 0, 0, "closed").await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(
        audit_count(&w.core, "container.exec_lingering", "error").await,
        1
    );
}

#[tokio::test]
async fn exec_rules_runtime_running_session_only_and_caps() {
    let w = world().await;
    let viewer = member(&w, "viewer", &[]).await;
    let runner = member(&w, "runner", &[Permission::Runtime]).await;
    assert_eq!(
        kind(w.svc.exec_open(&viewer, &w.shop, 80, 24).await),
        ErrorKind::Forbidden
    );
    assert_eq!(
        kind(w.svc.exec_open(&runner, &w.stray, 80, 24).await),
        ErrorKind::NotFound
    );
    // Tokens never open terminals (D24), even an Owner admin token.
    let owner_user = crate::testing::TestUser {
        id: String::new(),
        username: "owner".into(),
        password: crate::testing::OWNER_PASSWORD.into(),
    };
    let (_, tok) = w.core.token_actor(&owner_user, Scope::Admin).await;
    assert_eq!(
        kind(w.svc.exec_open(&tok, &w.shop, 80, 24).await),
        ErrorKind::Forbidden
    );

    // Stopped container: refused before anything is created.
    let stopped = w.core.docker.add_container("stopped", "exited", &[]);
    assert_eq!(
        kind(w.svc.exec_open(&w.owner, &stopped, 80, 24).await),
        ErrorKind::Conflict
    );
    assert!(
        !w.core
            .docker
            .calls()
            .iter()
            .any(|c| c.starts_with("exec_start") && c.contains("stopped"))
    );

    // Per-account cap, released when a session ends.
    let mut open = Vec::new();
    for _ in 0..EXEC_PER_ACCOUNT {
        open.push(w.svc.exec_open(&runner, &w.shop, 80, 24).await.unwrap());
    }
    assert_eq!(
        kind(w.svc.exec_open(&runner, &w.shop, 80, 24).await),
        ErrorKind::RateLimited
    );
    let (io, t) = open.pop().unwrap();
    t.finish(io.input, 0, 0, "closed").await;
    w.svc.exec_open(&runner, &w.shop, 80, 24).await.unwrap();
}
