//! End-to-end deployments against a real Docker daemon (and the network for the Git clone).
//! Ignored by default; run with `cargo test -p yanto-host --test docker_smoke -- --ignored`.
//! Everything it creates is removed afterwards (containers, networks, volumes, files).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use yanto_core::actor::Actor;
use yanto_core::deploy::{HostPorts, worker};
use yanto_core::password::Passwords;
use yanto_core::ports::SystemClock;
use yanto_core::secrets::Secrets;
use yanto_core::service::deployments::{DeployRequest, RollbackRequest};
use yanto_core::service::projects::{DeleteOptions, ProjectInput, ProjectPatch, SourceInput};
use yanto_core::service::{ServiceDeps, Services};
use yanto_core::{Config, Db};
use yanto_host::{ProcProbe, RunDirRegistryAuth, TokioCommands};

fn docker_available() -> bool {
    std::process::Command::new("docker")
        .args(["compose", "version"])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn compose(port: u16, marker: &str) -> String {
    format!(
        "services:\n  web:\n    image: nginx:alpine\n    environment:\n      - MARKER={marker}\n    ports:\n      - \"127.0.0.1:{port}:80\"\n"
    )
}

fn http_get(port: u16) -> String {
    for _ in 0..40 {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            s.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
                .unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            if out.starts_with("HTTP/1.") {
                return out;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    String::new()
}

/// Removes containers/networks/volumes of the smoke projects even when the test panics.
struct Cleanup(Vec<String>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for slug in &self.0 {
            let _ = std::process::Command::new("docker")
                .args([
                    "compose",
                    "-p",
                    slug,
                    "down",
                    "--volumes",
                    "--remove-orphans",
                ])
                .output();
        }
    }
}

fn input(name: &str, source: SourceInput, compose: String) -> ProjectInput {
    ProjectInput {
        name: name.into(),
        slug: Some(name.into()),
        source,
        compose_path: None,
        compose_content: Some(compose),
        env_file: None,
        auto_start: Some(true),
        manual_deploy_enabled: None,
        github_webhook_enabled: None,
        deploy_key_id: None,
        generate_deploy_key: false,
        registry_credential_id: None,
    }
}

#[tokio::test]
#[ignore = "needs a Docker daemon with the compose plugin, and network access"]
async fn git_and_image_projects_deploy_roll_back_and_clean_up() {
    if !docker_available() {
        eprintln!("docker compose is not available; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.paths.data_dir = dir.path().to_path_buf();
    let db = Db::open(&config.db_path()).await.unwrap();
    let services = Services::new(ServiceDeps {
        db: db.clone(),
        config: &config,
        clock: Arc::new(SystemClock),
        secrets: Arc::new(Secrets::ephemeral()),
        passwords: Passwords::cheap_for_tests(),
        docker: Arc::new(yanto_host::BollardDocker::new(&config.docker.socket)),
        host_stats: Arc::new(yanto_host::SysinfoStats::new()),
        host_mgmt: yanto_core::ports::HostMgmtPorts::unsupported(),
        host: HostPorts {
            command: Arc::new(TokioCommands),
            probe: Arc::new(ProcProbe),
            registry_auth: Arc::new(RunDirRegistryAuth::new(dir.path().join("run"))),
        },
    });
    let eng = services.engine();
    let cli = Actor::cli();
    let tag = yanto_core::crypto::random_hex::<3>();
    let (img_slug, git_slug, clash_slug) = (
        format!("yanto-smoke-img-{tag}"),
        format!("yanto-smoke-git-{tag}"),
        format!("yanto-smoke-clash-{tag}"),
    );
    let _cleanup = Cleanup(vec![img_slug.clone(), git_slug.clone(), clash_slug.clone()]);
    let (p_img, p_git) = (free_port(), free_port());

    // --- image-backed Project: deploy, serve, redeploy (no self collision), change, roll back.
    let img = services
        .projects
        .create(
            &cli,
            &input(
                &img_slug,
                SourceInput::Image {
                    image_ref: "nginx:alpine".into(),
                },
                compose(p_img, "one"),
            ),
        )
        .await
        .unwrap()
        .project;
    let d1 = services
        .deployments
        .deploy(&cli, &img.id, &DeployRequest::default())
        .await
        .unwrap()
        .deployment;
    assert!(worker::run_once(eng.clone()).await);
    let done = services.deployments.get(&cli, &d1.id).await.unwrap();
    let log = services.deployments.logs(&cli, &d1.id, None).await.unwrap();
    assert_eq!(done.status, "success", "{log}");
    assert!(
        done.image_digest
            .as_deref()
            .is_some_and(|d| d.contains("@sha256:")),
        "digest recorded"
    );
    assert!(
        http_get(p_img).contains("200 OK"),
        "nginx answers on 127.0.0.1:{p_img}"
    );
    assert_eq!(
        services
            .projects
            .ports_list(&cli, &img.id)
            .await
            .unwrap()
            .len(),
        1
    );

    // --- container -> Project mapping against the real Compose labels: a Member with `runtime`
    // on this Project sees and can restart its container, a Member of another Project and a
    // Member with view-only access cannot act, and unrelated containers on this Host stay hidden.
    {
        use std::collections::{BTreeMap, BTreeSet};
        use yanto_core::actor::{ActorKind, Channel, Permission, Role};
        let member = |perm: &[Permission], project: &str| {
            let mut grants = BTreeMap::new();
            grants.insert(
                project.to_owned(),
                perm.iter().copied().collect::<BTreeSet<_>>(),
            );
            Actor {
                kind: ActorKind::User,
                user_id: Some("usr_smoke".into()),
                label: "smoke".into(),
                role: Role::Member,
                grants,
                channel: Channel::Dashboard,
                ip: None,
                request_id: None,
            }
        };
        let runner = member(&[Permission::Runtime], &img.id);
        let viewer = member(&[], &img.id);
        let elsewhere = member(&[Permission::Runtime], "prj_other");
        let list = services.containers.list(&runner).await.unwrap().containers;
        assert_eq!(list.len(), 1, "only this Project's container: {list:?}");
        let web = list[0].clone();
        assert_eq!(web.project.as_deref(), Some(img.id.as_str()));
        assert_eq!(web.compose_project.as_deref(), Some(img_slug.as_str()));
        services.containers.restart(&runner, &web.id).await.unwrap();
        assert_eq!(
            services
                .containers
                .stop(&viewer, &web.id)
                .await
                .unwrap_err()
                .kind,
            yanto_core::ErrorKind::Forbidden
        );
        assert_eq!(
            services
                .containers
                .stop(&elsewhere, &web.id)
                .await
                .unwrap_err()
                .kind,
            yanto_core::ErrorKind::NotFound
        );
        assert!(
            services
                .containers
                .list(&elsewhere)
                .await
                .unwrap()
                .containers
                .is_empty()
        );
        assert!(
            services
                .containers
                .list(&cli)
                .await
                .unwrap()
                .containers
                .len()
                >= list.len(),
            "the Owner sees every container on the Host"
        );
        assert!(
            http_get(p_img).contains("200 OK"),
            "the container restarted by the Member serves again"
        );
    }

    let d2 = services
        .deployments
        .deploy(&cli, &img.id, &DeployRequest::default())
        .await
        .unwrap()
        .deployment;
    worker::run_once(eng.clone()).await;
    let again = services.deployments.get(&cli, &d2.id).await.unwrap();
    assert_eq!(
        again.status,
        "success",
        "redeploy must not collide with itself: {}",
        services.deployments.logs(&cli, &d2.id, None).await.unwrap()
    );

    services
        .projects
        .update(
            &cli,
            &img.id,
            &ProjectPatch {
                compose_content: Some(Some(compose(p_img, "two"))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let d3 = services
        .deployments
        .deploy(&cli, &img.id, &DeployRequest::default())
        .await
        .unwrap()
        .deployment;
    worker::run_once(eng.clone()).await;
    assert_eq!(
        services.deployments.get(&cli, &d3.id).await.unwrap().status,
        "success"
    );
    let preview = services
        .deployments
        .rollback_preview(&cli, &img.id, &RollbackRequest::default())
        .await;
    // Same image digest each time: nothing distinct to roll back to, unless the digest differs.
    if let Ok(pv) = preview {
        let rb = services
            .deployments
            .rollback(
                &cli,
                &img.id,
                &RollbackRequest {
                    deployment_id: pv.target.deployment_id,
                    git_ref: None,
                },
            )
            .await
            .unwrap();
        worker::run_once(eng.clone()).await;
        let rolled = services
            .deployments
            .get(&cli, &rb.deployment.id)
            .await
            .unwrap();
        assert_eq!(
            rolled.status,
            "success",
            "{}",
            services
                .deployments
                .logs(&cli, &rb.deployment.id, None)
                .await
                .unwrap()
        );
    }
    // Rolling back to an explicit earlier Deployment restores its exact compose snapshot.
    let rb = services
        .deployments
        .rollback(
            &cli,
            &img.id,
            &RollbackRequest {
                deployment_id: Some(d1.id.clone()),
                git_ref: None,
            },
        )
        .await
        .unwrap();
    worker::run_once(eng.clone()).await;
    let rolled = services
        .deployments
        .get(&cli, &rb.deployment.id)
        .await
        .unwrap();
    assert_eq!(
        rolled.status,
        "success",
        "{}",
        services
            .deployments
            .logs(&cli, &rb.deployment.id, None)
            .await
            .unwrap()
    );
    let on_disk = std::fs::read_to_string(
        dir.path()
            .join("projects")
            .join(&img_slug)
            .join("docker-compose.yml"),
    )
    .unwrap();
    assert!(
        on_disk.contains("MARKER=one") && !on_disk.contains("MARKER=two"),
        "{on_disk}"
    );

    // --- port collision with the running image Project is detected and fails the Deployment.
    let clash = services
        .projects
        .create(
            &cli,
            &input(
                &clash_slug,
                SourceInput::Image {
                    image_ref: "nginx:alpine".into(),
                },
                "services: {}\n".into(),
            ),
        )
        .await
        .unwrap()
        .project;
    services
        .projects
        .update(
            &cli,
            &clash.id,
            &ProjectPatch {
                compose_content: Some(Some(compose(p_img, "clash"))),
                ..Default::default()
            },
        )
        .await
        .map(|_| ())
        .expect_err("saving a compose that publishes a port owned by another project is a 409");

    // --- Git-backed Project: real https clone of a tiny public repository.
    let git = services
        .projects
        .create(
            &cli,
            &input(
                &git_slug,
                SourceInput::Git {
                    url: "https://github.com/octocat/Hello-World.git".into(),
                    branch: Some("master".into()),
                },
                compose(p_git, "git"),
            ),
        )
        .await
        .unwrap()
        .project;
    let g1 = services
        .deployments
        .deploy(&cli, &git.id, &DeployRequest::default())
        .await
        .unwrap()
        .deployment;
    worker::run_once(eng.clone()).await;
    let g = services.deployments.get(&cli, &g1.id).await.unwrap();
    let glog = services.deployments.logs(&cli, &g1.id, None).await.unwrap();
    assert_eq!(g.status, "success", "{glog}");
    assert!(
        g.commit_sha.as_deref().is_some_and(|s| s.len() == 40),
        "commit recorded"
    );
    assert!(
        dir.path()
            .join("projects")
            .join(&git_slug)
            .join(".git")
            .exists()
    );
    assert!(http_get(p_git).contains("200 OK"));
    // Second deploy pulls (fast-forward) instead of cloning.
    let g2 = services
        .deployments
        .deploy(&cli, &git.id, &DeployRequest::default())
        .await
        .unwrap()
        .deployment;
    worker::run_once(eng.clone()).await;
    let g2d = services.deployments.get(&cli, &g2.id).await.unwrap();
    assert_eq!(
        g2d.status,
        "success",
        "{}",
        services.deployments.logs(&cli, &g2.id, None).await.unwrap()
    );
    assert!(
        !services
            .deployments
            .logs(&cli, &g2.id, None)
            .await
            .unwrap()
            .contains("git clone")
    );
    // The SSH key directory is empty: no key was materialized (no deploy key here).
    assert!(
        !dir.path()
            .join("ssh")
            .join(&git_slug)
            .join("id_ed25519")
            .exists()
    );

    // --- Stop and delete (containers and volumes removed, files gone).
    services.projects.stop(&cli, &git.id).await.unwrap();
    for id in [&img.id, &git.id, &clash.id] {
        services
            .projects
            .delete(
                &cli,
                id,
                &DeleteOptions {
                    stop_containers: true,
                    delete_volumes: false,
                    confirm_slug: None,
                },
            )
            .await
            .unwrap();
    }
    let ps = std::process::Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("label=com.docker.compose.project={img_slug}"),
            "-q",
        ])
        .output()
        .unwrap();
    assert!(ps.stdout.is_empty(), "containers removed");
    assert!(!dir.path().join("projects").join(&img_slug).exists());
    db.close().await;
}
