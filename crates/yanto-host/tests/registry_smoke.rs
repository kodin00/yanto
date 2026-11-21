//! Hosted Registry against a real Docker daemon: enable on a loopback port, add a user,
//! `docker login`/`push`, browse, delete a tag, run GC, deploy a Project from it, disable.
//! Ignored by default; run with `cargo test -p yanto-host --test registry_smoke -- --ignored`.
//!
//! Everything is named `yanto-m6-smoke-<hex>` and removed afterwards (container, images pulled
//! by the test, Compose project, files). `docker login` uses a private `DOCKER_CONFIG`, so the
//! user's `~/.docker/config.json` is never touched.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

use yanto_core::actor::Actor;
use yanto_core::deploy::{HostPorts, worker};
use yanto_core::password::Passwords;
use yanto_core::ports::{RegistryDeps, SystemClock};
use yanto_core::secrets::Secrets;
use yanto_core::service::deployments::DeployRequest;
use yanto_core::service::projects::{ProjectInput, SourceInput};
use yanto_core::service::registry::{DELETE_DATA_PHRASE, EnableInput, GcState, Health};
use yanto_core::service::{ServiceDeps, Services};
use yanto_core::{Config, Db};
use yanto_host::{HttpRegistryApi, ProcProbe, RunDirRegistryAuth, TokioCommands};

const PORT: u16 = 5055;

fn docker(args: &[&str]) -> std::process::Output {
    Command::new("docker").args(args).output().expect("docker")
}

fn docker_cfg(cfg: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new("docker")
        .env("DOCKER_CONFIG", cfg)
        .args(args)
        .output()
        .expect("docker")
}

fn image_exists(image: &str) -> bool {
    docker(&["image", "inspect", image]).status.success()
}

/// Removes everything the test created, even when it panics.
struct Cleanup {
    container: String,
    compose_project: String,
    images: Vec<String>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.container]);
        let _ = docker(&[
            "compose",
            "-p",
            &self.compose_project,
            "down",
            "--volumes",
            "--remove-orphans",
        ]);
        for i in &self.images {
            let _ = docker(&["rmi", "-f", i]);
        }
    }
}

#[tokio::test]
#[ignore = "needs a Docker daemon (pulls registry:2 and busybox if absent)"]
async fn registry_end_to_end_with_a_real_daemon() {
    if !docker(&["info"]).status.success() {
        eprintln!("docker is not available; skipping");
        return;
    }
    let tag = yanto_core::crypto::random_hex::<3>();
    let prefix = format!("yanto-m6-smoke-{tag}");
    let host = format!("127.0.0.1:{PORT}");
    let repo = format!("{prefix}/busybox");
    let had_registry_image = image_exists("registry:2");
    let had_busybox = image_exists("busybox:latest");
    let mut cleanup = Cleanup {
        container: prefix.clone(),
        compose_project: format!("{prefix}-app"),
        images: vec![
            format!("{host}/{repo}:1"),
            format!("{host}/{repo}:latest"),
            format!("{host}/{repo}:2"),
        ],
    };
    if !had_busybox {
        cleanup.images.push("busybox:latest".into());
    }
    if !had_registry_image {
        cleanup.images.push("registry:2".into());
    }

    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.paths.data_dir = dir.path().join("data");
    std::fs::create_dir_all(&config.paths.data_dir).unwrap();
    let db = Db::open(&config.db_path()).await.unwrap();
    let services = Services::new(ServiceDeps {
        db: db.clone(),
        config: &config,
        clock: Arc::new(SystemClock),
        secrets: Arc::new(Secrets::ephemeral()),
        passwords: Passwords::cheap_for_tests(),
        docker: Arc::new(yanto_host::BollardDocker::new(&config.docker.socket)),
        host_stats: Arc::new(yanto_host::SysinfoStats::new()),
        registry: RegistryDeps::new(Arc::new(HttpRegistryApi::new())),
        host_mgmt: yanto_core::ports::HostMgmtPorts::unsupported(),
        host: HostPorts {
            command: Arc::new(TokioCommands),
            probe: Arc::new(ProcProbe),
            registry_auth: Arc::new(RunDirRegistryAuth::new(dir.path().join("run"))),
        },
    });
    sqlx::query("UPDATE hosted_registry_settings SET container_name = ?")
        .bind(&prefix)
        .execute(db.writer())
        .await
        .unwrap();
    let cli = Actor::cli();
    let reg = &services.registry;

    // --- enable: pinned image, loopback bind, auth required.
    let st = reg
        .enable(
            &cli,
            &EnableInput {
                listen_port: Some(PORT),
            },
        )
        .await
        .expect("enable");
    assert_eq!(st.health, Health::Healthy);
    let pinned = st.image_digest.clone().expect("pinned digest");
    assert!(pinned.contains("@sha256:"), "{pinned}");
    let bound = docker(&["port", &prefix, "5000/tcp"]);
    let bound = String::from_utf8_lossy(&bound.stdout).to_string();
    assert!(
        bound.contains(&format!("127.0.0.1:{PORT}")) && !bound.contains("0.0.0.0"),
        "{bound}"
    );
    let anon = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{PORT}/v2/"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        anon.status().as_u16(),
        401,
        "unauthenticated /v2/ is refused"
    );
    let inspect = docker(&[
        "inspect",
        "-f",
        "{{.HostConfig.RestartPolicy.Name}}",
        &prefix,
    ]);
    assert_eq!(
        String::from_utf8_lossy(&inspect.stdout).trim(),
        "unless-stopped"
    );

    // --- user + docker login (private DOCKER_CONFIG) and push.
    let user = reg.user_create(&cli, "smoke", None).await.expect("user");
    let pw = user.generated_password.clone().expect("generated");
    let cfg = dir.path().join("dockercfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let bad = Command::new("docker")
        .env("DOCKER_CONFIG", &cfg)
        .args(["login", &host, "-u", "smoke", "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut c| {
            c.stdin.take().unwrap().write_all(b"definitely-wrong-pw")?;
            c.wait()
        })
        .unwrap();
    assert!(!bad.success(), "wrong password must fail to log in");
    let mut login = Command::new("docker")
        .env("DOCKER_CONFIG", &cfg)
        .args(["login", &host, "-u", "smoke", "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    login
        .stdin
        .take()
        .unwrap()
        .write_all(pw.expose().as_bytes())
        .unwrap();
    let out = login.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "docker login: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        docker(&["pull", "busybox:latest"]).status.success(),
        "pull busybox"
    );
    for t in ["1", "latest", "2"] {
        let r = format!("{host}/{repo}:{t}");
        assert!(docker(&["tag", "busybox:latest", &r]).status.success());
    }
    for t in ["1", "latest"] {
        let r = format!("{host}/{repo}:{t}");
        let p = docker_cfg(&cfg, &["push", &r]);
        assert!(
            p.status.success(),
            "push {r}: {}",
            String::from_utf8_lossy(&p.stderr)
        );
    }

    // --- browse through the service.
    let repos = reg.repos_list(&cli, None, None).await.unwrap();
    assert!(repos.repos.contains(&repo), "{repos:?}");
    let tags = reg.tags_list(&cli, &repo, None, None).await.unwrap();
    let names: Vec<_> = tags.tags.iter().map(|t| t.tag.as_str()).collect();
    assert_eq!(names, ["1", "latest"]);
    assert_eq!(tags.tags[0].digest, tags.tags[1].digest);
    assert!(tags.tags[0].size.unwrap_or(0) > 0);

    // --- delete by digest: the sibling tag is listed first and goes too; blobs stay until GC.
    let preview = reg.tag_delete_preview(&cli, &repo, "1").await.unwrap();
    assert_eq!(preview.also_removed, ["latest"]);
    let blobs = dir.path().join("data/registry/data");
    let before_gc = dir_size(&blobs);
    assert!(before_gc > 0);
    reg.tag_delete(&cli, &repo, "1").await.expect("delete tag");
    assert!(
        reg.tags_list(&cli, &repo, None, None)
            .await
            .unwrap()
            .tags
            .is_empty()
    );
    // Only tag metadata goes; the blobs (nearly all the bytes) stay until GC.
    assert!(
        dir_size(&blobs) > before_gc / 10 * 9,
        "blobs stay on disk until GC"
    );
    let before_gc = dir_size(&blobs);

    // --- GC: dry run, real run, space reclaimed, writes work again afterwards.
    let dry = reg.gc_preview(&cli, true).await.expect("dry run");
    eprintln!("gc dry run output:\n{}", dry.output);
    reg.gc_start(&cli, true).await.expect("gc start");
    reg.gc_wait().await;
    let g = reg.gc_status(&cli).await.unwrap();
    assert_eq!(g.state, GcState::Done, "{:?}\n{}", g.error, g.output);
    assert!(
        g.reclaimed_bytes.unwrap_or(0) > 0,
        "GC reclaims space: {g:?}"
    );
    assert!(dir_size(&blobs) < before_gc);
    let cfg_text =
        std::fs::read_to_string(dir.path().join("data/registry/conf/config.yml")).unwrap();
    assert!(!cfg_text.contains("readonly"), "normal mode restored");
    let r2 = format!("{host}/{repo}:2");
    let p = docker_cfg(&cfg, &["push", &r2]);
    assert!(
        p.status.success(),
        "push after GC: {}",
        String::from_utf8_lossy(&p.stderr)
    );

    // --- a Project deploys the image without any manual login: remove the local copy first so
    // the pull really goes through the internal credential.
    for t in ["1", "latest", "2"] {
        let _ = docker(&["rmi", &format!("{host}/{repo}:{t}")]);
    }
    let slug = format!("{prefix}-app");
    let compose = format!(
        "services:\n  app:\n    image: {host}/{repo}:2\n    command: [\"sleep\", \"3600\"]\n"
    );
    let project = services
        .projects
        .create(
            &cli,
            &ProjectInput {
                name: slug.clone(),
                slug: Some(slug.clone()),
                source: SourceInput::Image {
                    image_ref: format!("{host}/{repo}:2"),
                },
                compose_path: None,
                compose_content: Some(compose),
                env_file: None,
                auto_start: Some(true),
                manual_deploy_enabled: None,
                github_webhook_enabled: None,
                deploy_key_id: None,
                generate_deploy_key: false,
                registry_credential_id: None,
            },
        )
        .await
        .unwrap()
        .project;
    let eng = services.engine();
    let dep = services
        .deployments
        .deploy(&cli, &project.id, &DeployRequest::default())
        .await
        .unwrap()
        .deployment;
    assert!(worker::run_once(eng).await);
    let done = services.deployments.get(&cli, &dep.id).await.unwrap();
    let log = services
        .deployments
        .logs(&cli, &dep.id, None)
        .await
        .unwrap();
    assert_eq!(done.status, "success", "{log}");
    assert!(!log.contains(pw.expose()));
    let running = docker(&[
        "ps",
        "--filter",
        &format!("label=com.docker.compose.project={slug}"),
        "-q",
    ]);
    assert!(!running.stdout.is_empty(), "the Project's container runs");
    let _ = docker(&[
        "compose",
        "-p",
        &slug,
        "down",
        "--volumes",
        "--remove-orphans",
    ]);

    // --- user removal denies access, then disable removes the container and the data.
    reg.user_delete(&cli, &user.user.id).await.unwrap();
    let _ = docker_cfg(&cfg, &["logout", &host]);
    let again = Command::new("docker")
        .env("DOCKER_CONFIG", &cfg)
        .args(["login", &host, "-u", "smoke", "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut c| {
            c.stdin.take().unwrap().write_all(pw.expose().as_bytes())?;
            c.wait()
        })
        .unwrap();
    assert!(!again.success(), "a removed user cannot log in");
    let off = reg.disable(&cli, Some(DELETE_DATA_PHRASE)).await.unwrap();
    assert!(!off.enabled);
    assert!(
        docker(&["ps", "-a", "--filter", &format!("name={prefix}"), "-q"])
            .stdout
            .is_empty(),
        "the container is gone"
    );
    assert!(!blobs.exists(), "data deleted with the typed phrase");
}

fn dir_size(p: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}
