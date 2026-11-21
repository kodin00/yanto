//! Deployments end to end against the fake command port: queue rules, the exact command
//! sequence, snapshots and rollback, cancellation, webhooks, recovery, log streaming.

use std::time::Duration;

use tokio_stream::StreamExt;

use super::projects::{git_input, image_input};
use super::{audit_count, kind};
use crate::ErrorKind;
use crate::actor::{Actor, Channel, Origin, Permission};
use crate::deploy::worker;
use crate::secrets::Secret;
use crate::service::deployments::{
    DeployRequest, GithubDelivery, GithubOutcome, LogStreamItem, RollbackRequest, github_signature,
};
use crate::service::projects::ProjectCreated;
use crate::testing::{Reply, TestCore};

struct Ctx {
    core: TestCore,
    owner: Actor,
}

async fn ctx() -> Ctx {
    let core = TestCore::new().await;
    let u = core.setup_owner().await;
    let owner = core.session_actor(&u).await;
    Ctx { core, owner }
}

impl Ctx {
    async fn image_project(&self, name: &str) -> ProjectCreated {
        self.core
            .services
            .projects
            .create(&self.owner, &image_input(name, "nginx:alpine"))
            .await
            .unwrap()
    }

    async fn deploy(&self, project: &str) -> String {
        self.core
            .services
            .deployments
            .deploy(&self.owner, project, &DeployRequest::default())
            .await
            .unwrap()
            .deployment
            .id
    }

    /// Run every claimable Deployment to completion.
    async fn run_all(&self) {
        let eng = self.core.services.engine();
        while worker::run_once(eng.clone()).await {}
    }

    async fn status(&self, id: &str) -> crate::service::deployments::DeploymentDto {
        self.core
            .services
            .deployments
            .get(&self.owner, id)
            .await
            .unwrap()
    }

    async fn logs(&self, id: &str) -> String {
        self.core
            .services
            .deployments
            .logs(&self.owner, id, None)
            .await
            .unwrap()
    }
}

fn origin() -> Origin {
    Origin::new(Channel::Webhook, Some("198.51.100.9".into()))
}

#[tokio::test]
async fn image_project_deploys_with_the_expected_commands_and_records_everything() {
    let c = ctx().await;
    let p = c.image_project("App").await.project;
    c.core
        .services
        .projects
        .env_write(&c.owner, &p.id, "SECRET_KEY=correct-horse-battery\n")
        .await
        .unwrap();
    let id = c.deploy(&p.id).await;
    assert_eq!(c.status(&id).await.status, "queued");
    c.run_all().await;

    let d = c.status(&id).await;
    assert_eq!(d.status, "success", "{d:?}");
    assert_eq!(d.exit_code, Some(0));
    assert!(
        d.image_digest
            .as_deref()
            .is_some_and(|x| x.contains("@sha256:"))
    );
    let calls = c.core.commands.calls();
    let pos = |needle: &str| {
        calls
            .iter()
            .position(|x| x.contains(needle))
            .unwrap_or_else(|| panic!("missing {needle} in {calls:#?}"))
    };
    assert!(pos("compose version") < pos("pull nginx:alpine"));
    assert!(pos("pull nginx:alpine") < pos("config --format json"));
    assert!(pos("config --format json") < pos("up -d --build --remove-orphans"));
    assert!(calls.iter().any(|x| x == "docker compose -p app -f docker-compose.yml -f .yanto.restart.override.yml up -d --build --remove-orphans"), "{calls:#?}");
    let log = c.logs(&id).await;
    assert!(log.contains("$ docker pull nginx:alpine") && log.contains("Deployment succeeded"));

    // The compose file is generated and snapshotted; the env snapshot is encrypted at rest.
    let dir = c.core.config.paths.data_dir.join("projects/app");
    assert!(
        std::fs::read_to_string(dir.join("docker-compose.yml"))
            .unwrap()
            .contains("image: nginx:alpine")
    );
    let (snap, env_enc): (String, String) =
        sqlx::query_as("SELECT compose_snapshot, env_snapshot_enc FROM deployments WHERE id = ?")
            .bind(&id)
            .fetch_one(c.core.db.reader())
            .await
            .unwrap();
    assert!(snap.contains("nginx:alpine"));
    assert!(env_enc.starts_with("enc:v1:") && !env_enc.contains("correct-horse"));
    assert_eq!(audit_count(&c.core, "deployment.finish", "ok").await, 1);
}

#[tokio::test]
async fn git_project_clones_with_the_deploy_key_and_removes_it_afterwards() {
    let c = ctx().await;
    let mut input = git_input("Repo", "git@github.com:org/repo.git");
    input.generate_deploy_key = true;
    let p = c
        .core
        .services
        .projects
        .create(&c.owner, &input)
        .await
        .unwrap()
        .project;
    let id = c.deploy(&p.id).await;
    c.run_all().await;
    let d = c.status(&id).await;
    assert_eq!(d.status, "success", "{d:?} {}", c.logs(&id).await);
    assert_eq!(
        d.commit_sha.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    assert_eq!(d.commit_message.as_deref(), Some("fake commit"));
    let clones = c.core.commands.matching("git clone");
    assert_eq!(clones.len(), 1);
    assert!(
        clones[0]
            .display
            .contains("--branch master -- git@github.com:org/repo.git .")
    );
    let env: std::collections::BTreeMap<_, _> = clones[0].spec.env.iter().cloned().collect();
    assert_eq!(env["GIT_TERMINAL_PROMPT"], "0");
    assert_eq!(env["GIT_ALLOW_PROTOCOL"], "ssh:https");
    let ssh = &env["GIT_SSH_COMMAND"];
    assert!(
        ssh.contains("IdentitiesOnly=yes")
            && ssh.contains("BatchMode=yes")
            && ssh.contains("StrictHostKeyChecking=accept-new")
    );
    let probe = clones[0].ssh_key.clone().expect("key probe");
    assert!(probe.exists, "the key file exists while git runs");
    #[cfg(unix)]
    assert_eq!(probe.mode, Some(0o600));
    assert!(!probe.path.exists(), "and is gone afterwards");
    // The private key never appears in argv or the log.
    assert!(
        c.core
            .commands
            .calls()
            .iter()
            .all(|x| !x.contains("OPENSSH"))
    );
    assert!(!c.logs(&id).await.contains("OPENSSH"));
}

#[tokio::test]
async fn second_deployment_pulls_instead_of_cloning() {
    let c = ctx().await;
    let p = c
        .core
        .services
        .projects
        .create(&c.owner, &git_input("R", "https://github.com/o/r.git"))
        .await
        .unwrap()
        .project;
    c.deploy(&p.id).await;
    c.run_all().await;
    // Simulate the clone having happened.
    std::fs::create_dir_all(c.core.config.paths.data_dir.join("projects/r/.git")).unwrap();
    c.core.commands.clear();
    c.deploy(&p.id).await;
    c.run_all().await;
    let calls = c.core.commands.calls();
    assert!(calls.iter().all(|x| !x.contains("git clone")));
    assert!(calls.iter().any(|x| x == "git fetch origin master"));
    assert!(
        calls
            .iter()
            .any(|x| x == "git pull --ff-only origin master")
    );
}

#[tokio::test]
async fn one_running_one_queued_and_the_third_trigger_reuses_the_queued_one() {
    let c = ctx().await;
    let p = c.image_project("Q").await.project;
    let d = &c.core.services.deployments;
    let first = d
        .deploy(&c.owner, &p.id, &DeployRequest::default())
        .await
        .unwrap();
    assert!(!first.reused);
    let eng = c.core.services.engine();
    // Claim the first (running), then trigger twice more while it "runs".
    let claimed = {
        let mut tx = c.core.db.begin_write().await.unwrap();
        let row = crate::db::repo::deployments::claim_next(&mut tx, 1)
            .await
            .unwrap()
            .unwrap();
        tx.commit().await.unwrap();
        row
    };
    assert_eq!(claimed.id, first.deployment.id);
    let second = d
        .deploy(&c.owner, &p.id, &DeployRequest::default())
        .await
        .unwrap();
    let third = d
        .deploy(&c.owner, &p.id, &DeployRequest::default())
        .await
        .unwrap();
    assert!(!second.reused && third.reused);
    assert_eq!(second.deployment.id, third.deployment.id);
    // A trigger carrying a payload is refused instead of being folded in and dropped.
    let with_env = DeployRequest {
        env: Some(crate::secrets::Secret::new("A=1\n".to_owned())),
        ..DeployRequest::default()
    };
    assert_eq!(
        kind(d.deploy(&c.owner, &p.id, &with_env).await),
        ErrorKind::Conflict
    );
    // While the first runs, the queued one cannot be claimed.
    let mut tx = c.core.db.begin_write().await.unwrap();
    assert!(
        crate::db::repo::deployments::claim_next(&mut tx, 2)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
    let running: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE status = 'running'")
            .fetch_one(c.core.db.reader())
            .await
            .unwrap();
    assert_eq!(running, 1);
    drop(eng);
}

#[tokio::test]
async fn cancel_queued_and_running() {
    let c = ctx().await;
    let p = c.image_project("C").await.project;
    let d = &c.core.services.deployments;
    let queued = c.deploy(&p.id).await;
    let out = d.cancel(&c.owner, &queued).await.unwrap();
    assert_eq!(out.status, "cancelled");
    assert_eq!(kind(d.cancel(&c.owner, &queued).await), ErrorKind::Conflict);

    // A row the worker claimed between the status read and the write is not relabelled.
    let raced = c.deploy(&p.id).await;
    sqlx::query("UPDATE deployments SET status = 'running' WHERE id = ?")
        .bind(&raced)
        .execute(c.core.db.writer())
        .await
        .unwrap();
    let mut tx = c.core.db.begin_write().await.unwrap();
    assert!(
        !crate::db::repo::deployments::cancel_queued(&mut tx, &raced, 1)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert_eq!(c.status(&raced).await.status, "running");
    sqlx::query("UPDATE deployments SET status = 'failed' WHERE id = ?")
        .bind(&raced)
        .execute(c.core.db.writer())
        .await
        .unwrap();

    // Running: the pull hangs until the cancellation token trips.
    c.core.commands.on("pull nginx:alpine", Reply::hang());
    let id = c.deploy(&p.id).await;
    let eng = c.core.services.engine();
    let run = tokio::spawn(async move { worker::run_once(eng).await });
    for _ in 0..200 {
        if c.status(&id).await.status == "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Let the runner reach the hanging command.
    for _ in 0..200 {
        if c.core
            .commands
            .calls()
            .iter()
            .any(|x| x.contains("pull nginx:alpine"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    d.cancel(&c.owner, &id).await.unwrap();
    run.await.unwrap();
    let done = c.status(&id).await;
    assert_eq!(
        (done.status.as_str(), done.exit_code),
        ("cancelled", Some(130))
    );
    assert!(c.logs(&id).await.contains("partially started stack"));
    assert_eq!(audit_count(&c.core, "deployment.cancel", "ok").await, 2);
}

#[tokio::test]
async fn failures_end_failed_with_a_reason() {
    let c = ctx().await;
    let p = c.image_project("F").await.project;
    c.core
        .commands
        .on("up -d", Reply::fail(1, "boom: port is already allocated"));
    let id = c.deploy(&p.id).await;
    c.run_all().await;
    let d = c.status(&id).await;
    assert_eq!(d.status, "failed");
    assert_eq!(d.exit_code, Some(1));
    assert!(
        d.error
            .as_deref()
            .unwrap()
            .contains("docker compose up failed (exit code 1)")
    );
    let log = c.logs(&id).await;
    assert!(log.contains("boom: port is already allocated") && log.contains("Deployment failed"));
    assert_eq!(audit_count(&c.core, "deployment.finish", "error").await, 1);

    // Compose plugin missing fails pre-flight.
    c.core.commands.on(
        "compose version",
        Reply::fail(1, "docker: 'compose' is not a docker command"),
    );
    let id = c.deploy(&p.id).await;
    c.run_all().await;
    assert!(
        c.status(&id)
            .await
            .error
            .unwrap()
            .contains("docker compose is not available")
    );
}

#[tokio::test]
async fn deployment_with_a_port_collision_fails_before_compose_up() {
    let c = ctx().await;
    let p = c.image_project("P").await.project;
    c.core.commands.on(
        "config --format json",
        Reply::stdout(r#"{"services":{"web":{"ports":[{"host_ip":"127.0.0.1","target":80,"published":"18090","protocol":"tcp"}]}}}"#),
    );
    c.core.probe.bind(18090, "tcp", Some("sshd"));
    let id = c.deploy(&p.id).await;
    c.run_all().await;
    let d = c.status(&id).await;
    assert_eq!(d.status, "failed");
    assert!(d.error.unwrap().contains("host process sshd"));
    assert!(c.core.commands.matching("up -d").is_empty());
}

#[tokio::test]
async fn ports_are_reconciled_and_redeploying_never_collides_with_itself() {
    let c = ctx().await;
    let p = c.image_project("Self").await.project;
    c.core.commands.on(
        "config --format json",
        Reply::stdout(r#"{"services":{"web":{"ports":[{"host_ip":"127.0.0.1","target":80,"published":"18091","protocol":"tcp"}]}}}"#),
    );
    let id = c.deploy(&p.id).await;
    c.run_all().await;
    assert_eq!(c.status(&id).await.status, "success");
    let ports = c
        .core
        .services
        .projects
        .ports_list(&c.owner, &p.id)
        .await
        .unwrap();
    assert_eq!(ports.len(), 1);
    // Now our own container publishes it and docker-proxy listens: still fine.
    c.core.probe.bind(18091, "tcp", Some("docker-proxy"));
    c.core.commands.on(
        "docker ps --format",
        Reply::stdout("self-web-1\tself\t127.0.0.1:18091->80/tcp\n"),
    );
    let id2 = c.deploy(&p.id).await;
    c.run_all().await;
    assert_eq!(
        c.status(&id2).await.status,
        "success",
        "{}",
        c.logs(&id2).await
    );
}

#[tokio::test]
async fn rollback_restores_the_exact_snapshotted_config_and_previews_the_diff() {
    let c = ctx().await;
    let p = c.image_project("Rb").await.project;
    let projects = &c.core.services.projects;
    projects
        .env_write(&c.owner, &p.id, "A=1\nSECRET=old-secret-value\n")
        .await
        .unwrap();
    let digest_a = "nginx@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let digest_b = "nginx@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    c.core
        .commands
        .on("image inspect", Reply::stdout(format!("[\"{digest_a}\"]")));
    let first = c.deploy(&p.id).await;
    c.run_all().await;

    // Change config and image, deploy again.
    projects
        .env_write(&c.owner, &p.id, "A=2\nB=new\n")
        .await
        .unwrap();
    projects
        .update(
            &c.owner,
            &p.id,
            &crate::service::projects::ProjectPatch {
                compose_content: Some(Some(
                    "services:\n  app:\n    image: nginx:alpine\n    environment:\n      - NEW=1\n"
                        .into(),
                )),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    c.core
        .commands
        .on("image inspect", Reply::stdout(format!("[\"{digest_b}\"]")));
    let second = c.deploy(&p.id).await;
    c.run_all().await;
    assert_eq!(
        c.status(&second).await.image_digest.as_deref(),
        Some(digest_b)
    );

    let d = &c.core.services.deployments;
    let preview = d
        .rollback_preview(&c.owner, &p.id, &RollbackRequest::default())
        .await
        .unwrap();
    assert_eq!(
        preview.target.deployment_id.as_deref(),
        Some(first.as_str())
    );
    assert_eq!(preview.target.image_digest.as_deref(), Some(digest_a));
    assert!(preview.restores_config);
    assert_eq!(preview.env_diff.removed, ["B"].map(String::from));
    assert_eq!(preview.env_diff.changed, ["A"].map(String::from));
    assert_eq!(preview.env_diff.added, ["SECRET"].map(String::from));
    assert!(
        preview
            .compose_diff
            .iter()
            .any(|l| l.kind == '-' && l.text.contains("NEW=1"))
    );
    assert!(
        !serde_json::to_string(&preview)
            .unwrap()
            .contains("old-secret-value"),
        "values stay masked"
    );

    c.core.commands.clear();
    let rb = d
        .rollback(&c.owner, &p.id, &RollbackRequest::default())
        .await
        .unwrap();
    assert_eq!(rb.deployment.trigger, "rollback");
    assert_eq!(
        rb.deployment.rollback_from_deployment_id.as_deref(),
        Some(first.as_str())
    );
    c.run_all().await;
    let done = c.status(&rb.deployment.id).await;
    assert_eq!(
        done.status,
        "success",
        "{}",
        c.logs(&rb.deployment.id).await
    );
    let dir = c.core.config.paths.data_dir.join("projects/rb");
    assert_eq!(
        std::fs::read_to_string(dir.join(".env")).unwrap(),
        "A=1\nSECRET=old-secret-value\n"
    );
    assert!(
        !std::fs::read_to_string(dir.join("docker-compose.yml"))
            .unwrap()
            .contains("NEW=1")
    );
    let calls = c.core.commands.calls();
    assert!(
        calls
            .iter()
            .any(|x| x == &format!("docker pull {digest_a}"))
    );
    assert!(
        calls
            .iter()
            .any(|x| x == &format!("docker tag {digest_a} nginx:alpine"))
    );
    assert!(
        calls.iter().all(|x| x != "docker pull nginx:alpine"),
        "no fresh pull on rollback"
    );
    assert_eq!(audit_count(&c.core, "deployment.rollback", "ok").await, 1);
}

#[tokio::test]
async fn git_rollback_checks_out_the_previous_commit() {
    let c = ctx().await;
    let p = c
        .core
        .services
        .projects
        .create(&c.owner, &git_input("G", "https://github.com/o/g.git"))
        .await
        .unwrap()
        .project;
    let sha = |n: char| n.to_string().repeat(40);
    c.core
        .commands
        .on("rev-parse HEAD", Reply::stdout(format!("{}\n", sha('a'))));
    c.deploy(&p.id).await;
    c.run_all().await;
    std::fs::create_dir_all(c.core.config.paths.data_dir.join("projects/g/.git")).unwrap();
    c.core
        .commands
        .on("rev-parse HEAD", Reply::stdout(format!("{}\n", sha('b'))));
    c.deploy(&p.id).await;
    c.run_all().await;
    c.core.commands.on("rev-list --count", Reply::stdout("3\n"));
    c.core.commands.on(
        "diff --numstat",
        Reply::stdout("1\t2\tsrc/main.rs\n5\t0\tREADME.md\n"),
    );
    let d = &c.core.services.deployments;
    let preview = d
        .rollback_preview(&c.owner, &p.id, &RollbackRequest::default())
        .await
        .unwrap();
    assert_eq!(preview.target.commit_sha, Some(sha('a')));
    assert_eq!(
        (preview.commits_behind, preview.commits_ahead),
        (Some(3), Some(3))
    );
    assert_eq!(preview.files.len(), 2);
    c.core.commands.clear();
    let rb = d
        .rollback(&c.owner, &p.id, &RollbackRequest::default())
        .await
        .unwrap();
    c.run_all().await;
    assert_eq!(c.status(&rb.deployment.id).await.status, "success");
    assert!(
        c.core
            .commands
            .calls()
            .iter()
            .any(|x| x == &format!("git checkout {} --", sha('a')))
    );
}

#[tokio::test]
async fn rollback_without_history_is_a_conflict() {
    let c = ctx().await;
    let p = c.image_project("None").await.project;
    let d = &c.core.services.deployments;
    assert_eq!(
        kind(
            d.rollback(&c.owner, &p.id, &RollbackRequest::default())
                .await
        ),
        ErrorKind::Conflict
    );
}

#[tokio::test]
async fn env_payload_needs_secrets_and_is_written_then_cleared() {
    let c = ctx().await;
    let p = c.image_project("Env").await.project;
    let m = c
        .core
        .create_member("dev", &[(&p.id, &[Permission::Deploy])])
        .await;
    let ma = c.core.session_actor(&m).await;
    let d = &c.core.services.deployments;
    let req = DeployRequest {
        target_ref: None,
        env: Some(Secret::new("TOKEN=abc-123-xyz\n")),
    };
    assert_eq!(kind(d.deploy(&ma, &p.id, &req).await), ErrorKind::Forbidden);
    let queued = d.deploy(&c.owner, &p.id, &req).await.unwrap().deployment.id;
    c.core.commands.on(
        "pull nginx:alpine",
        Reply {
            lines: vec!["leaking abc-123-xyz here".into()],
            ..Reply::ok()
        },
    );
    let pending: String =
        sqlx::query_scalar("SELECT pending_env_enc FROM deployments WHERE id = ?")
            .bind(&queued)
            .fetch_one(c.core.db.reader())
            .await
            .unwrap();
    assert!(pending.starts_with("enc:v1:") && !pending.contains("abc-123"));
    c.run_all().await;
    assert_eq!(c.status(&queued).await.status, "success");
    assert_eq!(
        std::fs::read_to_string(c.core.config.paths.data_dir.join("projects/env/.env")).unwrap(),
        "TOKEN=abc-123-xyz\n"
    );
    let after: Option<String> =
        sqlx::query_scalar("SELECT pending_env_enc FROM deployments WHERE id = ?")
            .bind(&queued)
            .fetch_one(c.core.db.reader())
            .await
            .unwrap();
    assert!(after.is_none(), "pending env is nulled once the run starts");
}

#[tokio::test]
async fn registry_credential_is_used_per_command_and_never_logged() {
    let c = ctx().await;
    let cred = c
        .core
        .services
        .registry_credentials
        .create(
            &c.owner,
            "ghcr",
            "https://GHCR.io/",
            "bot",
            &Secret::new("ghp_supersecrettoken123"),
        )
        .await
        .unwrap();
    assert_eq!(cred.registry_host, "ghcr.io");
    let mut input = image_input("Priv", "ghcr.io/org/app:1");
    input.registry_credential_id = Some(cred.id.clone());
    let p = c
        .core
        .services
        .projects
        .create(&c.owner, &input)
        .await
        .unwrap()
        .project;
    assert_eq!(
        kind(
            c.core
                .services
                .registry_credentials
                .delete(&c.owner, &cred.id)
                .await
        ),
        ErrorKind::Conflict
    );
    c.core.commands.on(
        "pull ghcr.io/org/app:1",
        Reply {
            lines: vec!["login with ghp_supersecrettoken123 failed?".into()],
            ..Reply::ok()
        },
    );
    let id = c.deploy(&p.id).await;
    c.run_all().await;
    assert_eq!(c.status(&id).await.status, "success");
    let created = c.core.registry_auth.created();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].1, ["ghcr.io:bot"]);
    assert!(
        c.core.registry_auth.live().is_empty(),
        "the temp DOCKER_CONFIG is removed afterwards"
    );
    let pulls = c.core.commands.matching("pull ghcr.io/org/app:1");
    let env: std::collections::BTreeMap<_, _> = pulls[0].spec.env.iter().cloned().collect();
    assert!(env["DOCKER_CONFIG"].contains(&format!("docker-{id}")));
    assert!(
        c.core
            .commands
            .calls()
            .iter()
            .all(|x| !x.contains("ghp_") && !x.contains("login")),
        "no docker login, no secret in argv"
    );
    let log = c.logs(&id).await;
    assert!(!log.contains("ghp_supersecrettoken123"), "redacted: {log}");
    let listing = serde_json::to_string(
        &c.core
            .services
            .registry_credentials
            .list(&c.owner)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(!listing.contains("ghp_") && !listing.contains("secret_enc"));
}

#[tokio::test]
async fn deploy_token_webhook_rules() {
    let c = ctx().await;
    let created = c.image_project("Hook").await;
    let (id, token) = (
        created.project.id.clone(),
        created.deploy_token.expose().to_owned(),
    );
    let d = &c.core.services.deployments;
    assert_eq!(
        kind(d.deploy_webhook(&origin(), &id, "ydp_wrong").await),
        ErrorKind::Unauthenticated
    );
    assert_eq!(
        kind(d.deploy_webhook(&origin(), "prj_unknown", &token).await),
        ErrorKind::Unauthenticated
    );
    assert_eq!(
        kind(d.deploy_webhook(&origin(), &id, "").await),
        ErrorKind::Unauthenticated
    );
    let ok = d.deploy_webhook(&origin(), &id, &token).await.unwrap();
    assert_eq!(ok.deployment.trigger, "deploy_token");
    assert!(!ok.reused);
    assert!(
        d.deploy_webhook(&origin(), &id, &token)
            .await
            .unwrap()
            .reused
    );
    assert!(audit_count(&c.core, "deployment.trigger", "denied").await >= 2);
    // Disabled manual deploys are 403, after the token proved valid.
    c.core
        .services
        .projects
        .update(
            &c.owner,
            &id,
            &crate::service::projects::ProjectPatch {
                manual_deploy_enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        kind(d.deploy_webhook(&origin(), &id, &token).await),
        ErrorKind::Forbidden
    );
    // A rotated token invalidates the old one at once.
    c.core
        .services
        .projects
        .update(
            &c.owner,
            &id,
            &crate::service::projects::ProjectPatch {
                manual_deploy_enabled: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    c.core
        .services
        .projects
        .deploy_token_rotate(&c.owner, &id)
        .await
        .unwrap();
    assert_eq!(
        kind(d.deploy_webhook(&origin(), &id, &token).await),
        ErrorKind::Unauthenticated
    );
}

#[tokio::test]
async fn deploy_webhook_is_rate_limited_per_ip_and_project() {
    let c = ctx().await;
    let created = c.image_project("Lim").await;
    let d = &c.core.services.deployments;
    for _ in 0..10 {
        let _ = d
            .deploy_webhook(&origin(), &created.project.id, "ydp_bad")
            .await;
    }
    let e = d
        .deploy_webhook(
            &origin(),
            &created.project.id,
            created.deploy_token.expose(),
        )
        .await
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::RateLimited);
    assert!(e.retry_after_secs.is_some());
}

#[tokio::test]
async fn github_webhook_flood_from_one_ip_does_not_block_github() {
    let c = ctx().await;
    let created = c
        .core
        .services
        .projects
        .create(&c.owner, &git_input("Gh", "https://github.com/o/gh.git"))
        .await
        .unwrap();
    let d = &c.core.services.deployments;
    let body = push("master");
    let call = |ip: &'static str, sig: Option<String>| {
        let (d, id, body) = (d.clone(), created.project.id.clone(), body.clone());
        async move {
            d.github_webhook(
                &Origin::new(Channel::Webhook, Some(ip.into())),
                &id,
                &GithubDelivery {
                    signature: sig.as_deref(),
                    event: Some("push"),
                    delivery_id: None,
                    form_encoded: false,
                    body: &body,
                },
            )
            .await
        }
    };
    for _ in 0..60 {
        let _ = call("203.0.113.66", None).await;
    }
    assert_eq!(
        kind(call("203.0.113.66", None).await),
        ErrorKind::RateLimited
    );
    let sig = github_signature(created.deploy_token.expose(), &body);
    assert!(call("192.30.252.1", Some(sig)).await.is_ok());
}

fn push(branch: &str) -> Vec<u8> {
    format!(
        r#"{{"ref":"refs/heads/{branch}","after":"{}","deleted":false}}"#,
        "c".repeat(40)
    )
    .into_bytes()
}

#[tokio::test]
async fn github_webhook_rules() {
    let c = ctx().await;
    let created = c
        .core
        .services
        .projects
        .create(&c.owner, &git_input("Gh", "https://github.com/o/gh.git"))
        .await
        .unwrap();
    let (id, token) = (
        created.project.id.clone(),
        created.deploy_token.expose().to_owned(),
    );
    let d = &c.core.services.deployments;
    let body = push("master");
    let sig = github_signature(&token, &body);
    let delivery = |sig: Option<&str>, event: &'static str, did: &'static str| {
        let sig = sig.map(str::to_owned);
        let body = body.clone();
        let d = d.clone();
        let id = id.clone();
        async move {
            d.github_webhook(
                &origin(),
                &id,
                &GithubDelivery {
                    signature: sig.as_deref(),
                    event: Some(event),
                    delivery_id: Some(did),
                    form_encoded: false,
                    body: &body,
                },
            )
            .await
        }
    };

    // Bad / missing signatures are 401 and never queue.
    let bad = github_signature("not-the-token", &body);
    assert_eq!(
        kind(delivery(Some(&bad), "push", "d0").await),
        ErrorKind::Unauthenticated
    );
    assert_eq!(
        kind(delivery(None, "push", "d0").await),
        ErrorKind::Unauthenticated
    );
    assert_eq!(
        kind(
            d.github_webhook(
                &origin(),
                "prj_unknown",
                &GithubDelivery {
                    signature: Some(&sig),
                    event: Some("push"),
                    delivery_id: None,
                    form_encoded: false,
                    body: &body
                }
            )
            .await
        ),
        ErrorKind::Unauthenticated
    );
    // ping, other events.
    assert!(matches!(
        delivery(Some(&sig), "ping", "d1").await.unwrap(),
        GithubOutcome::Pong
    ));
    assert!(matches!(
        delivery(Some(&sig), "issues", "d2").await.unwrap(),
        GithubOutcome::Ignored(_)
    ));
    // Another branch is ignored (signature computed for that body).
    let other = push("develop");
    let osig = github_signature(&token, &other);
    let r = d
        .github_webhook(
            &origin(),
            &id,
            &GithubDelivery {
                signature: Some(&osig),
                event: Some("push"),
                delivery_id: Some("d3"),
                form_encoded: false,
                body: &other,
            },
        )
        .await
        .unwrap();
    assert!(matches!(r, GithubOutcome::Ignored("other branch")));
    // Matching branch queues with trigger github and no target ref.
    let GithubOutcome::Queued(q) = delivery(Some(&sig), "push", "d4").await.unwrap() else {
        panic!("expected queued")
    };
    assert_eq!(
        (
            q.deployment.trigger.as_str(),
            q.deployment.target_ref.clone()
        ),
        ("github", None)
    );
    // The same delivery id again is a duplicate.
    assert!(matches!(
        delivery(Some(&sig), "push", "d4").await.unwrap(),
        GithubOutcome::Duplicate
    ));
    // Form-encoded bodies are unwrapped after verification; disabled webhooks are ignored.
    let form = format!(
        "payload={}",
        form_urlencoded::byte_serialize(&body).collect::<String>()
    )
    .into_bytes();
    let fsig = github_signature(&token, &form);
    let r = d
        .github_webhook(
            &origin(),
            &id,
            &GithubDelivery {
                signature: Some(&fsig),
                event: Some("push"),
                delivery_id: Some("d5"),
                form_encoded: true,
                body: &form,
            },
        )
        .await
        .unwrap();
    assert!(matches!(r, GithubOutcome::Queued(e) if e.reused));
    c.core
        .services
        .projects
        .update(
            &c.owner,
            &id,
            &crate::service::projects::ProjectPatch {
                github_webhook_enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        delivery(Some(&sig), "push", "d6").await.unwrap(),
        GithubOutcome::Ignored("github webhook disabled")
    ));
    // The audit row names the delivery, never the body.
    let meta: String = sqlx::query_scalar(
        "SELECT group_concat(metadata) FROM audit_logs WHERE actor_type = 'github'",
    )
    .fetch_one(c.core.db.reader())
    .await
    .unwrap();
    assert!(
        meta.contains("d4") && !meta.contains("\"deleted\""),
        "{meta}"
    );
}

#[tokio::test]
async fn startup_recovery_interrupts_running_and_keeps_queued() {
    let c = ctx().await;
    let p = c.image_project("Rec").await.project;
    let running = c.deploy(&p.id).await;
    {
        let mut tx = c.core.db.begin_write().await.unwrap();
        crate::db::repo::deployments::claim_next(&mut tx, 5)
            .await
            .unwrap()
            .unwrap();
        tx.commit().await.unwrap();
    }
    let queued = c.deploy(&p.id).await;
    let n = worker::recover(&c.core.services.engine()).await.unwrap();
    assert_eq!(n, 1);
    let r = c.status(&running).await;
    assert_eq!(r.status, "interrupted");
    assert!(r.error.unwrap().contains("Yanto restarted"));
    assert!(c.logs(&running).await.contains("Yanto restarted"));
    assert_eq!(c.status(&queued).await.status, "queued");
    // Queued work resumes.
    c.run_all().await;
    assert_eq!(c.status(&queued).await.status, "success");
}

#[tokio::test]
async fn log_stream_replays_resumes_without_gaps_and_closes_on_done() {
    let c = ctx().await;
    let p = c.image_project("Log").await.project;
    let id = c.deploy(&p.id).await;
    c.core.commands.on(
        "pull nginx:alpine",
        Reply {
            lines: (0..5).map(|i| format!("layer {i}")).collect(),
            ..Reply::ok()
        },
    );
    c.run_all().await;
    let d = &c.core.services.deployments;
    let collect = |from: Option<i64>| {
        let owner = c.owner.clone();
        let id = id.clone();
        async move {
            let mut s = d.log_stream(&owner, &id, from).await.unwrap();
            let mut seqs = Vec::new();
            let mut done = None;
            while let Some(item) = tokio::time::timeout(Duration::from_secs(5), s.next())
                .await
                .unwrap()
            {
                match item {
                    LogStreamItem::Log(c) => seqs.push(c.seq),
                    LogStreamItem::Done(s) => done = Some(s),
                    LogStreamItem::Status(_) => {}
                }
            }
            (seqs, done)
        }
    };
    let (all, done) = collect(None).await;
    assert_eq!(done.as_deref(), Some("success"));
    assert!(all.len() >= 2 && all.windows(2).all(|w| w[1] == w[0] + 1));
    let (rest, _) = collect(Some(all[0])).await;
    assert_eq!(
        rest,
        all[1..],
        "Last-Event-ID resumes without duplicates or gaps"
    );

    // A live stream attached before the run sees the logs and the end.
    let id2 = c.deploy(&p.id).await;
    let mut live = d.log_stream(&c.owner, &id2, None).await.unwrap();
    c.run_all().await;
    let mut got_log = false;
    let mut finished = None;
    while let Some(item) = tokio::time::timeout(Duration::from_secs(5), live.next())
        .await
        .unwrap()
    {
        match item {
            LogStreamItem::Log(_) => got_log = true,
            LogStreamItem::Done(s) => finished = Some(s),
            LogStreamItem::Status(_) => {}
        }
    }
    assert!(got_log);
    assert_eq!(finished.as_deref(), Some("success"));
}

#[tokio::test]
async fn a_deployment_whose_project_vanished_releases_its_cancel_token() {
    let c = ctx().await;
    let p = c.image_project("Gone").await.project;
    let id = c.deploy(&p.id).await;
    let mut row = {
        let mut conn = c.core.db.reader().acquire().await.unwrap();
        crate::db::repo::deployments::get(&mut conn, &id)
            .await
            .unwrap()
            .unwrap()
    };
    row.project_id = "prj_gone".into();
    let eng = c.core.services.engine();
    let token = eng.register_cancel(&id);
    crate::deploy::runner::execute(eng.clone(), row, token).await;
    assert!(!eng.cancel(&id), "token unregistered on the early return");
}

#[tokio::test]
async fn stored_logs_are_capped_with_a_truncation_marker() {
    let core = TestCore::with_config(|c| c.deploy.log_max_bytes = 600).await;
    let u = core.setup_owner().await;
    let owner = core.session_actor(&u).await;
    let c = Ctx { core, owner };
    let p = c.image_project("Cap").await.project;
    c.core.commands.on(
        "pull nginx:alpine",
        Reply {
            lines: (0..200)
                .map(|i| format!("noisy line number {i:04} with some padding to take space"))
                .collect(),
            ..Reply::ok()
        },
    );
    let id = c.deploy(&p.id).await;
    c.run_all().await;
    let d = c.status(&id).await;
    assert_eq!(d.status, "success");
    assert!(d.log_truncated, "{d:?}");
    let log = c.logs(&id).await;
    assert!(log.contains("[earlier log output was truncated]"));
    assert!(log.contains("Deployment succeeded"), "the tail is kept");
    assert!(log.len() < 2500, "{}", log.len());
    // Every flush past the cap truncates again, but only one marker survives.
    assert_eq!(
        log.matches("[earlier log output was truncated]").count(),
        1,
        "{log}"
    );
    let (stored, counted): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT SUM(length(CAST(data AS BLOB))) FROM deployment_logs WHERE deployment_id = ?1),
                (SELECT log_bytes FROM deployments WHERE id = ?1)",
    )
    .bind(&id)
    .fetch_one(c.core.db.reader())
    .await
    .unwrap();
    assert_eq!(stored, counted);
}

#[tokio::test]
async fn members_see_only_their_projects_deployments() {
    let c = ctx().await;
    let a = c.image_project("A").await.project;
    let b = c.image_project("B").await.project;
    let da = c.deploy(&a.id).await;
    let db = c.deploy(&b.id).await;
    let m = c
        .core
        .create_member("m", &[(&a.id, &[Permission::Deploy])])
        .await;
    let ma = c.core.session_actor(&m).await;
    let d = &c.core.services.deployments;
    let list = d.list(&ma, &Default::default()).await.unwrap();
    assert_eq!(
        list.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
        [da.as_str()]
    );
    assert_eq!(kind(d.get(&ma, &db).await), ErrorKind::NotFound);
    assert_eq!(kind(d.get(&ma, "dep_nope").await), ErrorKind::NotFound);
    assert_eq!(kind(d.cancel(&ma, &db).await), ErrorKind::NotFound);
    assert!(d.cancel(&ma, &da).await.is_ok());
    assert_eq!(kind(d.logs(&ma, &db, None).await), ErrorKind::NotFound);
}

#[tokio::test]
async fn non_fast_forward_pull_offers_an_audited_checkout_reset() {
    let c = ctx().await;
    let d = &c.core.services.deployments;
    let p = c
        .core
        .services
        .projects
        .create(&c.owner, &git_input("G", "https://github.com/o/g.git"))
        .await
        .unwrap()
        .project;
    // First deployment clones; the second finds a diverged checkout and fails at the pull.
    let id = c.deploy(&p.id).await;
    c.run_all().await;
    assert!(
        !c.status(&id).await.reset_offered,
        "successful runs offer nothing"
    );
    std::fs::create_dir_all(c.core.config.paths.data_dir.join("projects/g/.git")).unwrap();
    c.core.commands.on(
        "pull --ff-only",
        Reply::fail(128, "fatal: Not possible to fast-forward, aborting."),
    );
    let failed = c.deploy(&p.id).await;
    c.run_all().await;
    let dto = c.status(&failed).await;
    assert_eq!(dto.status, "failed");
    assert!(dto.reset_offered, "{:?}", dto.error);

    // Reset: fetch the branch, hard-reset to it, report the revisions, write the audit row.
    c.core.commands.clear();
    c.core.commands.on(
        "rev-parse HEAD",
        Reply::stdout(format!("{}\n", "c".repeat(40))),
    );
    let out = d.reset_checkout(&c.owner, &p.id).await.unwrap();
    assert_eq!(out.branch, "master");
    assert_eq!(out.to, "c".repeat(40));
    let calls = c.core.commands.calls();
    let pos = |needle: &str| {
        calls
            .iter()
            .position(|x| x.contains(needle))
            .unwrap_or_else(|| panic!("missing {needle} in {calls:#?}"))
    };
    assert!(
        pos("fetch origin master") < pos("reset --hard origin/master"),
        "{calls:#?}"
    );
    assert_eq!(
        audit_count(&c.core, "project.checkout_reset", "ok").await,
        1
    );
    let meta: String = sqlx::query_scalar(
        "SELECT metadata FROM audit_logs WHERE action = 'project.checkout_reset'",
    )
    .fetch_one(c.core.db.reader())
    .await
    .unwrap();
    assert!(
        meta.contains("\"from\"") && meta.contains("\"to\""),
        "{meta}"
    );

    // A failing fetch is reported and audited as an error; nothing else runs.
    c.core.commands.on(
        "fetch origin",
        Reply::fail(128, "fatal: could not read from remote"),
    );
    let e = d.reset_checkout(&c.owner, &p.id).await.unwrap_err();
    assert!(e.message.contains("git fetch failed"), "{}", e.message);
    assert_eq!(
        audit_count(&c.core, "project.checkout_reset", "error").await,
        1
    );

    // Image Projects have no checkout; Members need `deploy` on the Project.
    let img = c.image_project("Img").await.project;
    assert!(matches!(
        kind(d.reset_checkout(&c.owner, &img.id).await),
        ErrorKind::Validation(_)
    ));
    let viewer = c.core.create_member("viewer", &[(&p.id, &[])]).await;
    let viewer = c.core.session_actor(&viewer).await;
    assert_eq!(
        kind(d.reset_checkout(&viewer, &p.id).await),
        ErrorKind::Forbidden
    );
    let runner = c
        .core
        .create_member("runner", &[(&p.id, &[Permission::Deploy])])
        .await;
    let runner = c.core.session_actor(&runner).await;
    c.core.commands.on("fetch origin", Reply::ok());
    d.reset_checkout(&runner, &p.id).await.unwrap();
}
