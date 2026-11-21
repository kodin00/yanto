//! `yanto backup`, `yanto restore yanto`, `yanto cron-run` and the hidden `exec-as` helper,
//! end to end against a temp data dir and fixture host directories.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use yanto_core::actor::Actor;
use yanto_core::cron::{EntryInput, Overlap};
use yanto_core::password::Passwords;
use yanto_core::ports::{HostMgmtPorts, SystemClock};
use yanto_core::secrets::Secrets;
use yanto_core::service::{ServiceDeps, Services};
use yanto_core::{Config, Db};
use yanto_host::{HostAuthorizedKeys, HostBackup, HostCron, HostCronExec, SystemUsers};

struct Fx {
    dir: tempfile::TempDir,
    cfg: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir_all(p.join("etc/cron.d")).unwrap();
        let cfg = p.join("config.toml");
        std::fs::write(
            &cfg,
            format!(
                "config_version = 1\n[paths]\ndata_dir = \"{d}/data\"\n[host]\netc_dir = \"{d}/etc\"\nspool_dir = \"{d}/spool\"\nrun_dir = \"{d}/run\"\n",
                d = p.display()
            ),
        )
        .unwrap();
        Self { dir, cfg }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn yanto(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_yanto"))
            .args(args)
            .arg("--config")
            .arg(&self.cfg)
            .env_remove("YANTO_CONFIG")
            .env_remove("YANTO_CRON_RUN")
            .output()
            .expect("run yanto")
    }

    /// Creates the DB and `keys/app.key` (the daemon does this at first start).
    fn init(&self) {
        assert!(self.yanto(&["setup-code"]).status.success());
    }

    async fn services(&self) -> (Services, Db) {
        let config = Config::load(&self.cfg).unwrap();
        let db = Db::open(&config.db_path()).await.unwrap();
        let users = Arc::new(SystemUsers::default());
        let owner = (
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw(),
        );
        let h = &config.host;
        let s = Services::new(ServiceDeps {
            db: db.clone(),
            config: &config,
            clock: Arc::new(SystemClock),
            secrets: Arc::new(Secrets::ephemeral()),
            passwords: Passwords::cheap_for_tests(),
            docker: Arc::new(yanto_core::testing::docker::FakeDocker::new()),
            host_stats: Arc::new(yanto_core::testing::host_stats::FakeHostStats::new()),
            host: yanto_core::deploy::HostPorts {
                command: Arc::new(yanto_host::TokioCommands),
                probe: Arc::new(yanto_host::ProcProbe),
                registry_auth: Arc::new(yanto_host::RunDirRegistryAuth::new(
                    h.run_dir.join("deploy"),
                )),
            },
            host_mgmt: HostMgmtPorts {
                users: users.clone(),
                cron: Arc::new(
                    HostCron::new(&h.etc_dir, &h.spool_dir, owner).without_service_check(),
                ),
                cron_exec: Arc::new(HostCronExec::new(&h.run_dir)),
                authorized_keys: Arc::new(HostAuthorizedKeys::new(users)),
                backup_host: Arc::new(HostBackup::new(&h.run_dir)),
                project_backups: Arc::new(yanto_core::ports::UnavailableProjectBackups),
            },
        });
        (s, db)
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn current_user() -> String {
    let o = Command::new("id").arg("-un").output().unwrap();
    String::from_utf8(o.stdout).unwrap().trim().to_owned()
}

async fn code_hash(db_path: &Path) -> String {
    use sqlx::Connection;
    let mut c = sqlx::SqliteConnection::connect(&format!("sqlite://{}?mode=ro", db_path.display()))
        .await
        .unwrap();
    sqlx::query_scalar("SELECT value FROM app_settings WHERE key = 'setup.code_hash'")
        .fetch_one(&mut c)
        .await
        .unwrap()
}

#[test]
fn backup_keep_last_prunes_scheduled_only_and_lock_gives_exit_75() {
    let fx = Fx::new();
    fx.init();
    // A manual backup (no retention flags) is never pruned.
    let manual = fx.yanto(&["backup", "yanto"]);
    assert!(manual.status.success(), "{}", err(&manual));
    assert!(out(&manual).contains("-yanto.tar.zst") && out(&manual).contains("sha256:"));
    for _ in 0..3 {
        let o = fx.yanto(&["backup", "yanto", "--keep-last", "2"]);
        assert!(o.status.success(), "{}", err(&o));
    }
    let list = out(&fx.yanto(&["backup", "list"]));
    assert_eq!(
        list.lines().filter(|l| l.contains(" cron ")).count(),
        2,
        "{list}"
    );
    assert_eq!(
        list.lines().filter(|l| l.contains(" manual ")).count(),
        1,
        "{list}"
    );

    // A second concurrent run exits 75 while the lock is held.
    std::fs::create_dir_all(fx.dir.path().join("run")).unwrap();
    let lock = File::create(fx.dir.path().join("run/backup-yanto.lock")).unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let busy = fx.yanto(&["backup", "yanto"]);
    assert_eq!(busy.status.code(), Some(75), "{}", err(&busy));
    drop(lock);
    assert!(fx.yanto(&["backup", "yanto"]).status.success());
}

#[test]
fn backup_refuses_without_an_instance_key_and_creates_nothing() {
    let fx = Fx::new();
    let o = fx.yanto(&["backup", "yanto"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("app.key"), "{}", err(&o));
    assert!(!fx.data().join("keys").exists());
    assert!(!fx.data().join("yanto.db").exists());
}

#[tokio::test]
async fn project_backup_cli_resolves_the_slug_and_the_declaration() {
    let fx = Fx::new();
    fx.init();
    // Unknown Project.
    let o = fx.yanto(&["backup", "project", "blog", "--volumes"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("no Project with slug blog"), "{}", err(&o));
    // A flag is required, and `--stop-project` only goes with volumes.
    assert!(!fx.yanto(&["backup", "project", "blog"]).status.success());
    assert!(
        !fx.yanto(&["backup", "project", "blog", "--db", "db", "--stop-project"])
            .status
            .success()
    );
    // A Project without a declaration for the service.
    let (s, db) = fx.services().await;
    sqlx::query(
        "INSERT INTO projects (id, name, slug, source_kind, image_ref, deploy_token_hash, deploy_token_enc, created_at, updated_at)
         VALUES ('prj_cli', 'Blog', 'blog', 'image', 'nginx', 'h', 'e', 0, 0)",
    )
    .execute(db.writer())
    .await
    .unwrap();
    drop(s);
    db.close().await;
    let o = fx.yanto(&["backup", "project", "blog", "--db", "db"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("no database is declared"), "{}", err(&o));
}

#[tokio::test]
async fn deploy_enqueues_joins_the_queue_and_wait_can_time_out() {
    use yanto_core::service::projects::{ProjectInput, SourceInput};
    let fx = Fx::new();
    fx.init();
    assert!(!fx.yanto(&["deploy", "nope"]).status.success());
    let (s, db) = fx.services().await;
    s.projects
        .create(
            &yanto_core::actor::Actor::cli(),
            &ProjectInput {
                name: "Blog".into(),
                slug: Some("blog".into()),
                source: SourceInput::Image {
                    image_ref: "nginx:alpine".into(),
                },
                compose_path: None,
                compose_content: None,
                env_file: None,
                auto_start: None,
                manual_deploy_enabled: None,
                github_webhook_enabled: None,
                deploy_key_id: None,
                generate_deploy_key: false,
                registry_credential_id: None,
            },
        )
        .await
        .unwrap();
    drop(s);
    db.close().await;

    let first = fx.yanto(&["deploy", "blog"]);
    assert!(first.status.success(), "{}", err(&first));
    assert!(out(&first).contains("queued"), "{}", out(&first));
    let second = fx.yanto(&["deploy", "blog"]);
    assert!(out(&second).contains("joined"), "{}", out(&second));
    // A Git ref makes no sense for an image Project.
    assert!(
        !fx.yanto(&["deploy", "blog", "--ref", "main"])
            .status
            .success()
    );
    // Nobody runs the queue here, so waiting ends by timeout with exit code 124.
    let waited = fx.yanto(&["deploy", "blog", "--wait", "--timeout", "1"]);
    assert_eq!(waited.status.code(), Some(124), "{}", err(&waited));
    assert!(
        !fx.yanto(&["deploy", "blog", "--timeout", "5"])
            .status
            .success()
    );
}

#[tokio::test]
async fn restore_brings_back_the_backed_up_database_and_checks_the_archive() {
    let fx = Fx::new();
    fx.init();
    let db_path = fx.data().join("yanto.db");
    let before = code_hash(&db_path).await;
    let b = fx.yanto(&["backup", "yanto"]);
    assert!(b.status.success());
    let filename = out(&b).split_whitespace().nth(2).unwrap().to_owned();
    let archive = fx.data().join("backups/yanto").join(&filename);
    // Change state after the backup.
    assert!(fx.yanto(&["setup-code"]).status.success());
    assert_ne!(code_hash(&db_path).await, before);

    // A corrupted archive fails the checksum and changes nothing.
    let mut bytes = std::fs::read(&archive).unwrap();
    let n = bytes.len() / 2;
    bytes[n] ^= 0xff;
    let bad = fx.dir.path().join("bad.tar.zst");
    std::fs::write(&bad, bytes).unwrap();
    std::fs::copy(
        format!("{}.sha256", archive.display()),
        format!("{}.sha256", bad.display()),
    )
    .unwrap();
    let after_bad = code_hash(&db_path).await;
    let r = fx.yanto(&[
        "restore",
        "yanto",
        bad.to_str().unwrap(),
        "--yes",
        "--assume-stopped",
    ]);
    assert!(!r.status.success());
    assert!(
        format!("{:#?}{}", err(&r), out(&r)).contains("checksum"),
        "{}",
        err(&r)
    );
    assert_eq!(code_hash(&db_path).await, after_bad);

    let r = fx.yanto(&[
        "restore",
        "yanto",
        archive.to_str().unwrap(),
        "--yes",
        "--assume-stopped",
    ]);
    assert!(r.status.success(), "{}", err(&r));
    assert!(out(&r).contains("pre-restore") || out(&r).contains("restored"));
    assert_eq!(
        code_hash(&db_path).await,
        before,
        "database reverted to the backup"
    );
    let aside = std::fs::read_dir(fx.data()).unwrap().flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with("yanto.db.pre-restore-")
    });
    assert!(aside, "the replaced database is kept");
    // Without --yes and without a terminal the command refuses instead of restoring.
    assert!(
        !fx.yanto(&[
            "restore",
            "yanto",
            archive.to_str().unwrap(),
            "--assume-stopped"
        ])
        .status
        .success()
    );
}

fn entry(cmd: &str) -> EntryInput {
    EntryInput {
        name: "job".into(),
        schedule: "* * * * *".into(),
        run_as_user: current_user(),
        command: cmd.into(),
        overlap: Overlap::Allow,
        timeout_s: None,
        enabled: true,
    }
}

#[tokio::test]
async fn cron_run_executes_the_stored_command_and_records_the_run() {
    let fx = Fx::new();
    fx.init();
    let (s, db) = fx.services().await;
    let a = Actor::cli();
    let e = s
        .cron
        .create(&a, &entry("echo hello 100%; echo oops >&2; exit 4"))
        .await
        .unwrap();

    // The generated file wraps the entry; the command text is not in it.
    let file = std::fs::read_to_string(fx.dir.path().join("etc/cron.d/yanto")).unwrap();
    assert!(file.contains(&format!("cron-run {}", e.id)) && !file.contains("hello"));

    let r = fx.yanto(&["cron-run", &e.id]);
    assert_eq!(r.status.code(), Some(4), "{}{}", out(&r), err(&r));
    let runs = s.cron.runs_list(&a, &e.id).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (runs[0].status.as_str(), runs[0].exit_code),
        ("failed", Some(4))
    );
    let tail = runs[0].output_tail.as_deref().unwrap();
    assert!(
        tail.contains("hello 100%") && tail.contains("oops"),
        "{tail}"
    );
    assert!(runs[0].duration_ms.is_some());

    // A passing command exits 0.
    let ok = s.cron.update(&a, &e.id, &entry("true")).await.unwrap();
    assert!(fx.yanto(&["cron-run", &ok.id]).status.success());
    assert_eq!(
        s.cron.runs_list(&a, &e.id).await.unwrap()[0].status,
        "success"
    );

    // Unknown entry: an ordinary failure, not the "database unreadable" code.
    let missing = fx.yanto(&["cron-run", "crn_missing"]);
    assert!(!missing.status.success() && missing.status.code() != Some(75));
    db.close().await;
}

#[test]
fn cron_run_fails_closed_with_75_when_the_database_is_missing() {
    let fx = Fx::new();
    let r = fx.yanto(&["cron-run", "crn_any"]);
    assert_eq!(r.status.code(), Some(75), "{}", err(&r));
    assert!(
        !fx.data().join("yanto.db").exists(),
        "cron-run must not create a database"
    );
}

#[test]
fn exec_as_helper_refuses_to_run_without_privilege() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let fx = Fx::new();
    let o = fx.yanto(&[
        "exec-as",
        "--uid",
        "0",
        "--gid",
        "0",
        "--name",
        "root",
        "--",
        "/bin/echo",
        "x",
    ]);
    assert_eq!(o.status.code(), Some(126));
    assert!(!out(&o).contains('x'));
}
