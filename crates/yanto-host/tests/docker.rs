//! Host-integration tests against a real Docker daemon (docs/rewrite/08-conventions.md section 7).
//! Ignored by default; run with `cargo test -p yanto-host -- --ignored` where `docker info` works.
//! The socket is `$YANTO_TEST_DOCKER_SOCKET` or `/var/run/docker.sock`. Needs the `alpine` image
//! (pulled on demand) and the `docker` CLI to create and remove the throwaway container.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use yanto_core::ports::{DockerPort, PruneKind};
use yanto_host::BollardDocker;

fn socket() -> PathBuf {
    std::env::var_os("YANTO_TEST_DOCKER_SOCKET")
        .map_or_else(|| PathBuf::from("/var/run/docker.sock"), PathBuf::from)
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker")
        .args(args)
        .output()
        .expect("docker CLI");
    assert!(
        out.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Removes the container even when an assertion fails.
struct Throwaway(String);

impl Drop for Throwaway {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
    }
}

#[tokio::test]
#[ignore = "needs a Docker daemon"]
async fn list_logs_exec_resize_and_lifecycle_against_a_real_daemon() {
    let name = format!("yanto-m3-test-{}", std::process::id());
    docker(&["pull", "-q", "alpine:3"]);
    let id = docker(&[
        "run",
        "-d",
        "--name",
        &name,
        "--label",
        "com.docker.compose.project=m3test",
        "alpine:3",
        "sh",
        "-c",
        "echo hello-from-m3; sleep 600",
    ]);
    let _guard = Throwaway(name.clone());
    let d = BollardDocker::new(&socket());

    d.ping().await.unwrap();
    assert!(!d.info().await.unwrap().server_version.is_empty());

    let all = d.list_containers().await.unwrap();
    let me = all.iter().find(|c| c.name == name).expect("listed");
    assert!(me.is_running() && me.id.starts_with(&id[..12]));
    assert_eq!(me.compose_project(), Some("m3test"));

    tokio::time::sleep(Duration::from_millis(500)).await;
    let logs = d.logs(&me.id, 50).await.unwrap();
    assert!(
        logs.iter()
            .any(|l| l.text.contains("hello-from-m3") && l.timestamp.is_some())
    );
    let stats = d.container_stats(&me.id).await.unwrap().expect("stats");
    assert!(stats.mem_limit > 0);

    // Exec with a TTY: run a command, resize, hang up.
    let mut io = d.exec_start(&me.id, 100, 30).await.unwrap();
    io.input
        .write_all(b"echo exec-ok-$((20+22)); stty size\n")
        .await
        .unwrap();
    let mut seen = String::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(chunk)) = io.output.next().await {
            seen.push_str(&String::from_utf8_lossy(&chunk));
            if seen.contains("exec-ok-42") && seen.contains("30 100") {
                break;
            }
        }
    })
    .await;
    assert!(seen.contains("exec-ok-42"), "shell output: {seen:?}");
    assert!(seen.contains("30 100"), "initial size applied: {seen:?}");
    d.exec_resize(&io.exec_id, 120, 40).await.unwrap();
    io.input.write_all(b"stty size\n").await.unwrap();
    let mut seen2 = String::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(chunk)) = io.output.next().await {
            seen2.push_str(&String::from_utf8_lossy(&chunk));
            if seen2.contains("40 120") {
                break;
            }
        }
    })
    .await;
    assert!(seen2.contains("40 120"), "resize applied: {seen2:?}");
    io.input.shutdown().await.unwrap();
    drop(io.input);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!d.exec_inspect(&io.exec_id).await.unwrap().running);

    // Lifecycle and prune preview.
    d.stop_container(&me.id, 1).await.unwrap();
    let stopped = d.list_containers().await.unwrap();
    assert!(
        !stopped
            .iter()
            .find(|c| c.name == name)
            .unwrap()
            .is_running()
    );
    let preview = d.prune_preview().await.unwrap();
    assert!(preview.containers.iter().any(|c| c.name == name));
    d.start_container(&me.id).await.unwrap();
    assert!(
        d.list_containers()
            .await
            .unwrap()
            .iter()
            .any(|c| c.name == name && c.is_running())
    );
    d.restart_container(&me.id, 1).await.unwrap();

    // Never prune containers here: a developer's stopped containers would go with it.
    let _ = PruneKind::Containers;
    assert!(d.disk_usage().await.is_ok());
}
