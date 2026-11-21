//! Hosted Registry service: files, container commands, users, browse, delete, GC.

use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::Actor;
use crate::hosted_registry as hr;
use crate::ports::RegistryApiError;
use crate::secrets::Secret;
use crate::service::registry::{DELETE_DATA_PHRASE, EnableInput, GcState, Health};
use crate::testing::command::Reply;
use crate::testing::{TestCore, registry_api::fake_digest};

const PINNED: &str =
    "registry@sha256:1111111111111111111111111111111111111111111111111111111111111111";

fn cli() -> Actor {
    Actor::cli()
}

async fn enabled_core() -> TestCore {
    let core = TestCore::new().await;
    core.commands
        .on("image inspect", Reply::stdout(format!(r#"["{PINNED}"]"#)));
    core.services
        .registry
        .enable(
            &cli(),
            &EnableInput {
                listen_port: Some(5055),
            },
        )
        .await
        .expect("enable");
    core.commands.clear();
    core
}

fn htpasswd(core: &TestCore) -> String {
    std::fs::read_to_string(core.dir.path().join("registry/auth/htpasswd")).unwrap()
}

fn config(core: &TestCore) -> String {
    std::fs::read_to_string(core.dir.path().join("registry/conf/config.yml")).unwrap()
}

#[tokio::test]
async fn enable_writes_files_pins_the_image_and_runs_a_loopback_container() {
    let core = TestCore::new().await;
    core.commands
        .on("image inspect", Reply::stdout(format!(r#"["{PINNED}"]"#)));
    let st = core
        .services
        .registry
        .enable(
            &cli(),
            &EnableInput {
                listen_port: Some(5055),
            },
        )
        .await
        .unwrap();
    assert!(st.enabled);
    assert_eq!(st.listen_port, 5055);
    assert_eq!(st.image_host, "127.0.0.1:5055");
    assert_eq!(st.image_digest.as_deref(), Some(PINNED));
    assert_eq!(st.health, Health::Healthy);

    let calls = core.commands.calls();
    assert!(calls[0].starts_with("docker pull registry:2"), "{calls:?}");
    let run = calls.iter().find(|c| c.starts_with("docker run")).unwrap();
    assert!(run.contains("-p 127.0.0.1:5055:5000"), "{run}");
    assert!(run.contains("--restart unless-stopped"));
    assert!(run.contains("--label yanto.managed=registry"));
    assert!(run.contains(&format!("{PINNED} serve /etc/yanto/config.yml")));
    assert!(run.contains(":/auth:ro") && run.contains(":/var/lib/registry"));

    // htpasswd holds exactly the internal user, bcrypt `$2y$`, mode 0600.
    let h = htpasswd(&core);
    assert!(h.starts_with("yanto-internal:$2y$04$"), "{h}");
    assert_eq!(h.lines().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(core.dir.path().join("registry/auth/htpasswd"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let c = config(&core);
    assert!(c.contains("delete:\n    enabled: true") && !c.contains("readonly"));
    assert_eq!(audit_count(&core, "registry.enable", "ok").await, 1);

    assert_eq!(
        kind(
            core.services
                .registry
                .enable(&cli(), &EnableInput::default())
                .await
        ),
        ErrorKind::Conflict
    );
}

#[tokio::test]
async fn the_internal_password_reaches_the_api_and_is_never_in_audit_rows() {
    let core = enabled_core().await;
    let s = core.services.registry.status(&cli()).await.unwrap();
    assert!(s.enabled);
    let (enc,): (String,) =
        sqlx::query_as("SELECT internal_password_enc FROM hosted_registry_settings")
            .fetch_one(core.db.reader())
            .await
            .unwrap();
    let pw = core
        .services
        .ctx()
        .secrets
        .open(
            "hosted_registry_settings",
            "internal_password_enc",
            "1",
            &enc,
        )
        .unwrap();
    core.registry_api.expect_password(Some(pw.expose()));
    core.services
        .registry
        .repos_list(&cli(), None, None)
        .await
        .unwrap();
    let meta: Vec<(String,)> = sqlx::query_as("SELECT metadata FROM audit_logs")
        .fetch_all(core.db.reader())
        .await
        .unwrap();
    assert!(meta.iter().all(|(m,)| !m.contains(pw.expose())));
}

#[tokio::test]
async fn enable_rejects_bad_ports_and_failed_starts_leave_nothing_behind() {
    let core = TestCore::new().await;
    let reg = &core.services.registry;
    let e = |p| EnableInput {
        listen_port: Some(p),
    };
    assert!(matches!(
        kind(reg.enable(&cli(), &e(80)).await),
        ErrorKind::Validation(_)
    ));
    let own = core.config.server.port;
    assert!(matches!(
        kind(reg.enable(&cli(), &e(own)).await),
        ErrorKind::Validation(_)
    ));
    core.probe.bind(5056, "tcp", Some("nginx"));
    let err = reg.enable(&cli(), &e(5056)).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    assert!(err.message.contains("nginx"), "{}", err.message);
    core.ensure_project("prj_a").await;
    sqlx::query("INSERT INTO project_ports (id, project_id, label, host_port, container_port, bind_address, created_at) VALUES ('ppt_a','prj_a','web',5057,80,'127.0.0.1',0)")
        .execute(core.db.writer())
        .await
        .unwrap();
    assert_eq!(
        reg.enable(&cli(), &e(5057)).await.unwrap_err().kind,
        ErrorKind::Conflict
    );

    // The container never answers: the half-started container is removed and nothing is enabled.
    core.registry_api
        .set_ping_error(Some(RegistryApiError::Unreachable("refused".into())));
    let err = reg.enable(&cli(), &e(5058)).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unavailable);
    assert!(!reg.status(&cli()).await.unwrap().enabled);
    assert!(
        core.commands
            .calls()
            .last()
            .unwrap()
            .starts_with("docker rm -f yanto-registry"),
        "{:?}",
        core.commands.calls()
    );
}

#[tokio::test]
async fn a_foreign_container_with_the_name_blocks_enable() {
    let core = TestCore::new().await;
    core.docker.add_container("yanto-registry", "running", &[]);
    let err = core
        .services
        .registry
        .enable(&cli(), &EnableInput::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    assert!(
        core.commands
            .calls()
            .iter()
            .all(|c| !c.starts_with("docker run"))
    );
}

#[tokio::test]
async fn users_are_hashed_into_htpasswd_and_the_registry_restarts() {
    let core = enabled_core().await;
    let reg = &core.services.registry;
    let created = reg.user_create(&cli(), "ci-bot", None).await.unwrap();
    let pw = created.generated_password.expect("generated once");
    let h = htpasswd(&core);
    let line = h.lines().find(|l| l.starts_with("ci-bot:")).unwrap();
    let hash = line.split_once(':').unwrap().1;
    assert!(hash.starts_with("$2y$"));
    assert!(hr::verify_password(pw.expose(), hash));
    assert!(!h.contains(pw.expose()));
    assert!(
        core.commands
            .calls()
            .iter()
            .any(|c| c.starts_with("docker restart"))
    );

    // A chosen password is not echoed back; policy and name rules apply.
    let chosen = Secret::new("a-chosen-password-1");
    let u2 = reg
        .user_create(&cli(), "second", Some(&chosen))
        .await
        .unwrap();
    assert!(u2.generated_password.is_none());
    assert!(matches!(
        kind(
            reg.user_create(&cli(), "x", Some(&Secret::new("short")))
                .await
        ),
        ErrorKind::Validation(_)
    ));
    assert!(matches!(
        kind(reg.user_create(&cli(), "Bad:Name", None).await),
        ErrorKind::Validation(_)
    ));
    assert!(matches!(
        kind(reg.user_create(&cli(), "yanto-internal", None).await),
        ErrorKind::Validation(_)
    ));
    assert_eq!(
        kind(reg.user_create(&cli(), "ci-bot", None).await),
        ErrorKind::Conflict
    );

    // Password change replaces the hash; delete removes the line.
    let new_pw = reg
        .user_password_set(&cli(), &created.user.id, None)
        .await
        .unwrap()
        .unwrap();
    let h2 = htpasswd(&core);
    let hash2 = h2
        .lines()
        .find(|l| l.starts_with("ci-bot:"))
        .unwrap()
        .split_once(':')
        .unwrap()
        .1;
    assert!(hr::verify_password(new_pw.expose(), hash2));
    assert!(!hr::verify_password(pw.expose(), hash2));
    reg.user_delete(&cli(), &created.user.id).await.unwrap();
    assert!(!htpasswd(&core).contains("ci-bot:"));
    assert!(htpasswd(&core).contains("second:"));
    assert_eq!(reg.users_list(&cli()).await.unwrap().len(), 1);
    assert_eq!(audit_count(&core, "registry_user.create", "ok").await, 2);
    let metas: Vec<(String,)> =
        sqlx::query_as("SELECT metadata FROM audit_logs WHERE action LIKE 'registry_user.%'")
            .fetch_all(core.db.reader())
            .await
            .unwrap();
    assert!(
        metas
            .iter()
            .all(|(m,)| !m.contains(pw.expose()) && !m.contains("a-chosen-password-1"))
    );
}

#[tokio::test]
async fn users_can_be_managed_while_disabled_without_touching_docker() {
    let core = TestCore::new().await;
    let reg = &core.services.registry;
    reg.user_create(&cli(), "early", None).await.unwrap();
    assert!(core.commands.calls().is_empty());
    assert!(!core.dir.path().join("registry/auth/htpasswd").exists());
    // Enabling renders the existing users.
    reg.enable(
        &cli(),
        &EnableInput {
            listen_port: Some(5055),
        },
    )
    .await
    .unwrap();
    assert!(htpasswd(&core).contains("early:"));
}

#[tokio::test]
async fn browsing_pages_beyond_100_and_reports_size_and_digest() {
    let core = enabled_core().await;
    let api = &core.registry_api;
    for i in 0..250 {
        api.push(
            "team/app",
            &format!("t{i:03}"),
            &fake_digest(&format!("d{i}")),
            1000,
        );
    }
    api.push("other", "latest", &fake_digest("o"), 5);
    let reg = &core.services.registry;
    let repos = reg.repos_list(&cli(), None, None).await.unwrap();
    assert_eq!(repos.repos, vec!["other", "team/app"]);
    assert!(repos.next.is_none());
    let small = reg.repos_list(&cli(), None, Some(1)).await.unwrap();
    assert_eq!(small.next.as_deref(), Some("other"));
    let p1 = reg.tags_list(&cli(), "team/app", None, None).await.unwrap();
    assert_eq!(p1.tags.len(), 100);
    assert_eq!(p1.tags[0].size, Some(1100));
    assert!(p1.tags[0].digest.starts_with("sha256:"));
    let p2 = reg
        .tags_list(&cli(), "team/app", p1.next.as_deref(), None)
        .await
        .unwrap();
    let p3 = reg
        .tags_list(&cli(), "team/app", p2.next.as_deref(), None)
        .await
        .unwrap();
    assert_eq!((p2.tags.len(), p3.tags.len()), (100, 50));
    assert!(p3.next.is_none());
    assert!(matches!(
        kind(reg.tags_list(&cli(), "Bad Repo", None, None).await),
        ErrorKind::Validation(_)
    ));
}

#[tokio::test]
async fn browse_errors_map_to_status_codes() {
    let core = enabled_core().await;
    let reg = &core.services.registry;
    core.registry_api
        .set_ping_error(Some(RegistryApiError::Unauthorized));
    assert_eq!(
        kind(reg.repos_list(&cli(), None, None).await),
        ErrorKind::Unavailable
    );
    let st = reg.status(&cli()).await.unwrap();
    assert_eq!(st.health, Health::AuthMismatch);
    core.registry_api
        .set_ping_error(Some(RegistryApiError::Unreachable("x".into())));
    assert!(matches!(
        reg.status(&cli()).await.unwrap().health,
        Health::Down(_)
    ));
    core.registry_api.set_ping_error(None);
    assert_eq!(
        kind(reg.tags_list(&cli(), "nope", None, None).await),
        ErrorKind::NotFound
    );
    // Disabled registry: browse is a 409.
    let off = TestCore::new().await;
    assert_eq!(
        kind(off.services.registry.repos_list(&cli(), None, None).await),
        ErrorKind::Conflict
    );
}

#[tokio::test]
async fn tag_delete_lists_siblings_and_deletes_by_digest() {
    let core = enabled_core().await;
    let api = &core.registry_api;
    let d = fake_digest("same");
    api.push("app", "v1", &d, 10);
    api.push("app", "latest", &d, 10);
    api.push("app", "v2", &fake_digest("other"), 10);
    let reg = &core.services.registry;
    let p = reg.tag_delete_preview(&cli(), "app", "v1").await.unwrap();
    assert_eq!(p.digest, d);
    assert_eq!(p.also_removed, vec!["latest"]);
    assert_eq!(api.tags_of("app").len(), 3, "preview deletes nothing");
    let done = reg.tag_delete(&cli(), "app", "v1").await.unwrap();
    assert_eq!(done.also_removed, vec!["latest"]);
    assert_eq!(api.tags_of("app"), vec!["v2"]);
    assert!(api.calls().iter().any(|c| c == &format!("delete app@{d}")));
    assert_eq!(audit_count(&core, "registry.tag_delete", "ok").await, 1);
    assert_eq!(
        kind(reg.tag_delete(&cli(), "app", "v1").await),
        ErrorKind::NotFound
    );
    api.set_delete_disabled(true);
    assert_eq!(
        kind(reg.tag_delete(&cli(), "app", "v2").await),
        ErrorKind::Conflict
    );
}

#[tokio::test]
async fn gc_preview_runs_a_dry_run_exec() {
    let core = enabled_core().await;
    core.commands.on(
        "garbage-collect --dry-run",
        Reply {
            lines: vec!["blob eligible for deletion".into()],
            ..Default::default()
        },
    );
    let p = core
        .services
        .registry
        .gc_preview(&cli(), true)
        .await
        .unwrap();
    assert!(p.output.contains("eligible"));
    let calls = core.commands.calls();
    assert_eq!(
        calls.last().unwrap(),
        "docker exec yanto-registry registry garbage-collect --dry-run --delete-untagged /etc/yanto/config.yml"
    );
}

#[tokio::test]
async fn gc_goes_read_only_collects_and_restores_normal_mode() {
    let core = enabled_core().await;
    core.commands.on(
        "registry garbage-collect --delete-untagged",
        Reply {
            lines: vec!["deleting blob".into()],
            ..Default::default()
        },
    );
    let reg = &core.services.registry;
    let started = reg.gc_start(&cli(), true).await.unwrap();
    assert_eq!(started.state, GcState::Running);
    reg.gc_wait().await;
    let st = reg.gc_status(&cli()).await.unwrap();
    assert_eq!(st.state, GcState::Done, "{:?}", st.error);
    assert!(st.output.contains("deleting blob"));
    let calls = core.commands.calls();
    let order: Vec<&str> = calls
        .iter()
        .map(|c| {
            if c.contains("garbage-collect") {
                "gc"
            } else if c.starts_with("docker restart") {
                "restart"
            } else {
                "other"
            }
        })
        .collect();
    assert_eq!(order, vec!["restart", "gc", "restart"], "{calls:?}");
    // Back to the writable configuration, flag cleared, audited.
    assert!(!config(&core).contains("readonly"));
    let s = reg.status(&cli()).await.unwrap();
    assert!(!s.gc_in_progress && !s.gc_left_readonly);
    assert_eq!(audit_count(&core, "registry.gc", "ok").await, 1);
}

#[tokio::test]
async fn a_failed_gc_still_restores_normal_mode() {
    let core = enabled_core().await;
    core.commands.on("garbage-collect", Reply::fail(1, "boom"));
    let reg = &core.services.registry;
    reg.gc_start(&cli(), false).await.unwrap();
    reg.gc_wait().await;
    let st = reg.gc_status(&cli()).await.unwrap();
    assert_eq!(st.state, GcState::Failed);
    assert!(st.error.unwrap().contains("exited"));
    assert!(!config(&core).contains("readonly"));
    let restarts = core
        .commands
        .calls()
        .iter()
        .filter(|c| c.starts_with("docker restart"))
        .count();
    assert_eq!(restarts, 2, "read-only restart and the restore restart");
    assert!(!reg.status(&cli()).await.unwrap().gc_left_readonly);
    assert_eq!(audit_count(&core, "registry.gc", "error").await, 1);
}

#[tokio::test]
async fn recover_restores_a_collection_interrupted_by_a_restart() {
    let core = enabled_core().await;
    // Simulate a crash mid-GC: read-only config on disk and the flag set.
    std::fs::write(
        core.dir.path().join("registry/conf/config.yml"),
        hr::render_config(true),
    )
    .unwrap();
    sqlx::query("UPDATE hosted_registry_settings SET gc_in_progress = 1")
        .execute(core.db.writer())
        .await
        .unwrap();
    let reg = &core.services.registry;
    assert!(reg.status(&cli()).await.unwrap().gc_left_readonly);
    assert!(reg.recover().await.unwrap());
    assert!(!config(&core).contains("readonly"));
    assert!(
        core.commands
            .calls()
            .iter()
            .any(|c| c.starts_with("docker restart"))
    );
    assert!(!reg.status(&cli()).await.unwrap().gc_left_readonly);
    assert!(!reg.recover().await.unwrap(), "nothing left to recover");
}

#[tokio::test]
async fn disable_removes_the_container_and_optionally_the_data() {
    let core = enabled_core().await;
    let data = core.dir.path().join("registry/data");
    std::fs::write(data.join("blob"), b"x").unwrap();
    let reg = &core.services.registry;
    assert!(matches!(
        kind(reg.disable(&cli(), Some("yes")).await),
        ErrorKind::Validation(_)
    ));
    let st = reg.disable(&cli(), None).await.unwrap();
    assert!(!st.enabled);
    assert!(data.join("blob").exists(), "data kept by default");
    assert!(
        core.commands
            .calls()
            .iter()
            .any(|c| c.starts_with("docker rm -f yanto-registry"))
    );
    assert_eq!(kind(reg.disable(&cli(), None).await), ErrorKind::Conflict);
    // Enable again keeps the pinned digest (no second pull) and the data; then wipe on disable.
    core.commands.clear();
    reg.enable(&cli(), &EnableInput::default()).await.unwrap();
    assert!(
        core.commands
            .calls()
            .iter()
            .all(|c| !c.starts_with("docker pull"))
    );
    reg.disable(&cli(), Some(DELETE_DATA_PHRASE)).await.unwrap();
    assert!(!data.exists());
    assert_eq!(audit_count(&core, "registry.disable", "ok").await, 2);
}

#[tokio::test]
async fn public_host_is_validated_and_stored() {
    let core = TestCore::new().await;
    let reg = &core.services.registry;
    let st = reg
        .settings_update(&cli(), Some("https://Registry.Example.com/"))
        .await
        .unwrap();
    assert_eq!(st.public_host.as_deref(), Some("registry.example.com"));
    assert!(matches!(
        kind(reg.settings_update(&cli(), Some("bad host")).await),
        ErrorKind::Validation(_)
    ));
    assert!(
        reg.settings_update(&cli(), Some(""))
            .await
            .unwrap()
            .public_host
            .is_none()
    );
}

// ---- Projects deploying from the Hosted Registry (04 4.4) ----

use super::projects::{git_input, image_input};
use crate::deploy::worker;
use crate::service::deployments::DeployRequest;

async fn deploy_and_run(core: &TestCore, project: &str) -> (String, String) {
    let id = core
        .services
        .deployments
        .deploy(&cli(), project, &DeployRequest::default())
        .await
        .unwrap()
        .deployment
        .id;
    let eng = core.services.engine();
    while worker::run_once(eng.clone()).await {}
    let status = core
        .services
        .deployments
        .get(&cli(), &id)
        .await
        .unwrap()
        .status;
    let log = core
        .services
        .deployments
        .logs(&cli(), &id, None)
        .await
        .unwrap();
    (status, log)
}

#[tokio::test]
async fn an_image_on_the_hosted_registry_gets_the_internal_credential_for_the_pull() {
    let core = enabled_core().await;
    let p = core
        .services
        .projects
        .create(&cli(), &image_input("Local", "127.0.0.1:5055/team/app:1"))
        .await
        .unwrap()
        .project;
    let (status, log) = deploy_and_run(&core, &p.id).await;
    assert_eq!(status, "success", "{log}");
    let created = core.registry_auth.created();
    assert_eq!(created.len(), 1);
    assert_eq!(
        created[0].1,
        [
            "127.0.0.1:5055:yanto-internal",
            "localhost:5055:yanto-internal"
        ]
    );
    assert!(core.registry_auth.live().is_empty());
    let pulls = core.commands.matching("pull 127.0.0.1:5055/team/app:1");
    let env: std::collections::BTreeMap<_, _> = pulls[0].spec.env.iter().cloned().collect();
    assert!(env.contains_key("DOCKER_CONFIG"));
    let secrets = core.registry_auth.secrets_seen();
    assert!(!secrets.is_empty());
    assert!(
        secrets.iter().all(|s| !log.contains(s.as_str())),
        "internal password stays out of logs"
    );
    // No `docker login` anywhere.
    assert!(core.commands.calls().iter().all(|c| !c.contains("login")));
}

#[tokio::test]
async fn other_images_get_no_hosted_registry_credential() {
    let core = enabled_core().await;
    let p = core
        .services
        .projects
        .create(&cli(), &image_input("Other", "nginx:alpine"))
        .await
        .unwrap()
        .project;
    let (status, log) = deploy_and_run(&core, &p.id).await;
    assert_eq!(status, "success", "{log}");
    assert!(core.registry_auth.created().is_empty());
}

#[tokio::test]
async fn a_git_project_whose_compose_names_the_registry_gets_the_credential_after_the_clone() {
    let core = enabled_core().await;
    let mut input = git_input("Repo", "git@github.com:org/repo.git");
    input.compose_content = Some("services:\n  app:\n    image: 127.0.0.1:5055/app:1\n".into());
    let p = core
        .services
        .projects
        .create(&cli(), &input)
        .await
        .unwrap()
        .project;
    let (status, log) = deploy_and_run(&core, &p.id).await;
    assert_eq!(status, "success", "{log}");
    assert_eq!(core.registry_auth.created().len(), 1);
    assert!(!core.commands.matching("compose -p").is_empty());
    assert!(
        core.commands
            .calls()
            .iter()
            .any(|c| c.contains("pull --ignore-buildable"))
    );
    assert!(core.registry_auth.live().is_empty());
}

#[tokio::test]
async fn pulling_from_a_disabled_hosted_registry_fails_with_a_hint() {
    let core = TestCore::new().await;
    let p = core
        .services
        .projects
        .create(&cli(), &image_input("Local", "127.0.0.1:5000/app:1"))
        .await
        .unwrap()
        .project;
    core.commands.on(
        "pull 127.0.0.1:5000/app:1",
        Reply::fail(1, "connection refused"),
    );
    let (status, log) = deploy_and_run(&core, &p.id).await;
    assert_eq!(status, "failed");
    assert!(log.contains("Hosted Registry, which is disabled"), "{log}");
    assert!(core.registry_auth.created().is_empty());
}
