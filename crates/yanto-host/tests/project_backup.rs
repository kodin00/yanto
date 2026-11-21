//! Project volume archives, Postgres/MariaDB dumps and restores against a real Docker daemon.
//! Ignored by default; run with `cargo test -p yanto-host --test project_backup -- --ignored`.
//! Everything it creates (containers, networks, volumes, and any image it had to pull) is removed
//! afterwards, even when an assertion fails.

use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use yanto_core::actor::Actor;
use yanto_core::backup::BackupKind;
use yanto_core::deploy::{HostPorts, worker};
use yanto_core::password::Passwords;
use yanto_core::ports::{HostMgmtPorts, SystemClock};
use yanto_core::secrets::Secrets;
use yanto_core::service::backups::{BackupDto, RestoreOptions};
use yanto_core::service::deployments::DeployRequest;
use yanto_core::service::project_databases::DatabaseInput;
use yanto_core::service::projects::{ProjectInput, SourceInput};
use yanto_core::service::{ServiceDeps, Services};
use yanto_core::{Config, Db};
use yanto_host::{DockerProjectBackups, HostBackup, ProcProbe, RunDirRegistryAuth, TokioCommands};

const PG_IMAGE: &str = "postgres:16-alpine";
const MARIA_IMAGE: &str = "mariadb:11";
const HELPER: &str = "busybox:stable";

fn docker(args: &[&str]) -> std::process::Output {
    Command::new("docker").args(args).output().expect("docker")
}

fn docker_ok(args: &[&str]) -> String {
    let o = docker(args);
    assert!(
        o.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn image_present(image: &str) -> bool {
    docker(&["image", "inspect", image]).status.success()
}

/// Removes the Project's containers, networks and volumes, and the images this run pulled.
struct Cleanup {
    slug: String,
    pulled: Vec<String>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = docker(&[
            "compose",
            "-p",
            &self.slug,
            "down",
            "--volumes",
            "--remove-orphans",
        ]);
        // Volumes created by restores carry the Compose project label but are not in the file.
        let label = format!("label=com.docker.compose.project={}", self.slug);
        for v in
            String::from_utf8_lossy(&docker(&["volume", "ls", "-q", "--filter", &label]).stdout)
                .lines()
        {
            let _ = docker(&["volume", "rm", "--force", v]);
        }
        for img in &self.pulled {
            let _ = docker(&["image", "rm", img]);
        }
    }
}

fn exec(slug: &str, service: &str, script: &str) -> String {
    let id = docker_ok(&[
        "ps",
        "-q",
        "--filter",
        &format!("label=com.docker.compose.project={slug}"),
        "--filter",
        &format!("label=com.docker.compose.service={service}"),
    ]);
    let id = id.trim();
    assert!(!id.is_empty(), "service {service} is not running");
    docker_ok(&["exec", id, "sh", "-c", script])
}

fn wait_for(slug: &str, service: &str, script: &str) {
    for _ in 0..90 {
        let id = docker_ok(&[
            "ps",
            "-q",
            "--filter",
            &format!("label=com.docker.compose.project={slug}"),
            "--filter",
            &format!("label=com.docker.compose.service={service}"),
        ]);
        let id = id.trim();
        if !id.is_empty() && docker(&["exec", id, "sh", "-c", script]).status.success() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    panic!("{service} did not become ready");
}

fn running_count(slug: &str) -> usize {
    docker_ok(&[
        "ps",
        "-q",
        "--filter",
        &format!("label=com.docker.compose.project={slug}"),
        "--filter",
        "status=running",
    ])
    .lines()
    .count()
}

fn unzstd(path: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    zstd::Decoder::new(std::fs::File::open(path).unwrap())
        .unwrap()
        .read_to_end(&mut out)
        .unwrap();
    out
}

/// `(path, contents)` of the regular files in a volume archive.
fn tar_files(path: &Path) -> Vec<(String, String)> {
    let mut ar = tar::Archive::new(zstd::Decoder::new(std::fs::File::open(path).unwrap()).unwrap());
    let mut v = Vec::new();
    for e in ar.entries().unwrap() {
        let mut e = e.unwrap();
        if e.header().entry_type() == tar::EntryType::Regular {
            let name = e.path().unwrap().to_string_lossy().into_owned();
            let mut s = String::new();
            let _ = e.read_to_string(&mut s);
            v.push((name, s));
        }
    }
    v
}

fn file_of(data: &Path, b: &BackupDto) -> std::path::PathBuf {
    let dir = match &b.project_id {
        Some(_) => data.join("backups/projects"),
        None => data.join("backups/yanto"),
    };
    // projects/<slug>/<file>
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path().join(&b.filename);
        if p.exists() {
            return p;
        }
    }
    panic!("{} not found under {}", b.filename, dir.display());
}

#[tokio::test]
#[ignore = "needs a Docker daemon (and pulls busybox and mariadb when missing)"]
async fn project_volume_and_database_backups_end_to_end() {
    if !docker(&["compose", "version"]).status.success() {
        eprintln!("docker compose is not available; skipping");
        return;
    }
    let tag = yanto_core::crypto::random_hex::<3>();
    let slug = format!("ym2x-bk-{tag}");
    let pulled: Vec<String> = [PG_IMAGE, MARIA_IMAGE, HELPER]
        .into_iter()
        .filter(|i| !image_present(i))
        .map(str::to_owned)
        .collect();
    let _cleanup = Cleanup {
        slug: slug.clone(),
        pulled,
    };

    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.paths.data_dir = dir.path().to_path_buf();
    let db = Db::open(&config.db_path()).await.unwrap();
    let mut host = HostMgmtPorts::unsupported();
    host.backup_host = Arc::new(HostBackup::new(dir.path().join("lock")));
    host.project_backups = Arc::new(DockerProjectBackups::default());
    let services = Services::new(ServiceDeps {
        db: db.clone(),
        config: &config,
        clock: Arc::new(SystemClock),
        secrets: Arc::new(Secrets::ephemeral()),
        passwords: Passwords::cheap_for_tests(),
        docker: Arc::new(yanto_host::BollardDocker::new(&config.docker.socket)),
        host_stats: Arc::new(yanto_host::SysinfoStats::new()),
        host_mgmt: host,
        host: HostPorts {
            command: Arc::new(TokioCommands),
            probe: Arc::new(ProcProbe),
            registry_auth: Arc::new(RunDirRegistryAuth::new(dir.path().join("run"))),
        },
    });
    let cli = Actor::cli();
    let compose = format!(
        "services:\n  \
           db:\n    image: {PG_IMAGE}\n    environment:\n      POSTGRES_PASSWORD: pg-test-pass\n      POSTGRES_DB: app\n    volumes:\n      - pgdata:/var/lib/postgresql/data\n  \
           maria:\n    image: {MARIA_IMAGE}\n    environment:\n      MARIADB_ROOT_PASSWORD: maria-test-pass\n      MARIADB_DATABASE: shop\n  \
           files:\n    image: {HELPER}\n    command: [\"sleep\", \"3600\"]\n    volumes:\n      - vol_a:/a\n      - vol_b:/b\n\
         volumes:\n  pgdata:\n  vol_a:\n  vol_b:\n"
    );
    let project = services
        .projects
        .create(
            &cli,
            &ProjectInput {
                name: slug.clone(),
                slug: Some(slug.clone()),
                source: SourceInput::Image {
                    image_ref: HELPER.into(),
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
    let d = services
        .deployments
        .deploy(&cli, &project.id, &DeployRequest::default())
        .await
        .unwrap()
        .deployment;
    assert!(worker::run_once(services.engine()).await);
    let done = services.deployments.get(&cli, &d.id).await.unwrap();
    assert_eq!(
        done.status,
        "success",
        "{}",
        services.deployments.logs(&cli, &d.id, None).await.unwrap()
    );
    wait_for(&slug, "db", "pg_isready -U postgres -d app");
    wait_for(
        &slug,
        "maria",
        "MYSQL_PWD=maria-test-pass $(command -v mariadb || command -v mysql) -u root -e 'select 1'",
    );

    // Seed data: Postgres rows, MariaDB rows, files in two volumes.
    exec(
        &slug,
        "db",
        "psql -U postgres -d app -c \"create table t(x int); insert into t values (1),(2),(3)\"",
    );
    exec(
        &slug,
        "maria",
        "MYSQL_PWD=maria-test-pass $(command -v mariadb || command -v mysql) -u root shop -e 'create table t(x int); insert into t values (1),(2)'",
    );
    exec(
        &slug,
        "files",
        "echo alpha > /a/one.txt; echo beta > /b/two.txt",
    );

    services
        .projects
        .databases_set(
            &cli,
            &project.id,
            &[
                DatabaseInput {
                    engine: "postgres".into(),
                    compose_service: "db".into(),
                    ..Default::default()
                },
                DatabaseInput {
                    engine: "mysql".into(),
                    compose_service: "maria".into(),
                    ..Default::default()
                },
            ],
        )
        .await
        .unwrap();
    let b = &services.backups;
    let data = dir.path();

    // --- volumes: one archive each; the Project keeps running; contents are right.
    let before = running_count(&slug);
    let vols = b
        .create_project(&cli, &project.id, BackupKind::ProjectVolumes, None, false)
        .await
        .unwrap();
    assert_eq!(vols.len(), 3, "pgdata, vol_a and vol_b: {vols:?}");
    assert_eq!(running_count(&slug), before, "the Project keeps running");
    let vol_a = vols
        .iter()
        .find(|v| v.note.as_deref() == Some(&format!("volume={slug}_vol_a")))
        .expect("vol_a archive");
    let files = tar_files(&file_of(data, vol_a));
    assert!(
        files
            .iter()
            .any(|(n, c)| n.ends_with("one.txt") && c.trim() == "alpha"),
        "{files:?}"
    );
    let vol_b = vols
        .iter()
        .find(|v| v.note.as_deref() == Some(&format!("volume={slug}_vol_b")))
        .unwrap();
    assert!(
        tar_files(&file_of(data, vol_b))
            .iter()
            .any(|(n, _)| n.ends_with("two.txt"))
    );

    // --- database dumps from credentials that exist only in the container environment.
    let pg = b
        .create_project(
            &cli,
            &project.id,
            BackupKind::ProjectPostgres,
            Some("db"),
            false,
        )
        .await
        .unwrap()
        .remove(0);
    let sql = String::from_utf8(unzstd(&file_of(data, &pg))).unwrap();
    assert!(
        sql.contains("CREATE TABLE public.t") && sql.contains("COPY public.t"),
        "{sql}"
    );
    assert!(!sql.contains("pg-test-pass"), "no password in the dump");
    let my = b
        .create_project(
            &cli,
            &project.id,
            BackupKind::ProjectMysql,
            Some("maria"),
            false,
        )
        .await
        .unwrap()
        .remove(0);
    let sql = String::from_utf8(unzstd(&file_of(data, &my))).unwrap();
    assert!(
        sql.contains("CREATE TABLE `t`") && sql.contains("INSERT INTO `t`"),
        "{sql}"
    );

    // --- stop-project guard: stopped during the archive, running again afterwards.
    b.create_project(
        &cli,
        &project.id,
        BackupKind::ProjectVolumes,
        Some(&format!("{slug}_vol_b")),
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        running_count(&slug),
        before,
        "started again after the backup"
    );
    wait_for(&slug, "db", "pg_isready -U postgres -d app");
    wait_for(
        &slug,
        "maria",
        "MYSQL_PWD=maria-test-pass $(command -v mariadb || command -v mysql) -u root -e 'select 1'",
    );

    // --- a volume outside the Project is refused (and never created).
    let foreign = b
        .create_project(
            &cli,
            &project.id,
            BackupKind::ProjectVolumes,
            Some("ym2x-not-mine"),
            false,
        )
        .await;
    assert!(foreign.is_err(), "volumes outside the Project are refused");

    // --- restore a database dump: drop the rows, restore, rows are back.
    exec(&slug, "db", "psql -U postgres -d app -c 'delete from t'");
    exec(
        &slug,
        "maria",
        "MYSQL_PWD=maria-test-pass $(command -v mariadb || command -v mysql) -u root shop -e 'drop table t'",
    );
    let confirm = Some(slug.clone());
    let out = b
        .restore(
            &cli,
            &pg.id,
            RestoreOptions {
                in_place: true,
                confirm_name: confirm.clone(),
            },
        )
        .await
        .unwrap();
    assert!(out.pre_restore.is_some());
    assert_eq!(
        exec(
            &slug,
            "db",
            "psql -U postgres -d app -tAc 'select count(*) from t'"
        )
        .trim(),
        "3"
    );
    b.restore(
        &cli,
        &my.id,
        RestoreOptions {
            in_place: true,
            confirm_name: confirm.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        exec(&slug, "maria", "MYSQL_PWD=maria-test-pass $(command -v mariadb || command -v mysql) -u root shop -N -e 'select count(*) from t'").trim(),
        "2"
    );

    // --- restore a volume: default creates a new volume and leaves the original alone.
    exec(&slug, "files", "echo changed > /a/one.txt");
    let nv = b
        .restore(&cli, &vol_a.id, RestoreOptions::default())
        .await
        .unwrap()
        .new_volume
        .unwrap();
    let copy = docker_ok(&[
        "run",
        "--rm",
        "--network",
        "none",
        "-v",
        &format!("{nv}:/data:ro"),
        HELPER,
        "cat",
        "/data/one.txt",
    ]);
    assert_eq!(
        copy.trim(),
        "alpha",
        "the new volume holds the archived data"
    );
    assert_eq!(
        exec(&slug, "files", "cat /a/one.txt").trim(),
        "changed",
        "original untouched"
    );

    // --- in place: pre-restore backup, Project stopped and started, data back.
    let r = b
        .restore(
            &cli,
            &vol_a.id,
            RestoreOptions {
                in_place: true,
                confirm_name: confirm,
            },
        )
        .await
        .unwrap();
    assert_eq!(r.pre_restore.unwrap().trigger, "pre_restore");
    assert_eq!(
        running_count(&slug),
        before,
        "started again after the restore"
    );
    assert_eq!(exec(&slug, "files", "cat /a/one.txt").trim(), "alpha");

    // No helper containers left behind.
    assert_eq!(
        docker_ok(&["ps", "-aq", "--filter", "label=yanto.managed=backup-helper"]).trim(),
        ""
    );
    assert!(
        !docker(&["volume", "inspect", "ym2x-not-mine"])
            .status
            .success()
    );
    db.close().await;
}
