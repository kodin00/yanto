//! `yanto setup-code` and `yanto owner-reset` end to end against a temp data dir.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use yanto_core::actor::{Channel, Origin};
use yanto_core::password::Passwords;
use yanto_core::ports::SystemClock;
use yanto_core::secrets::Secrets;
use yanto_core::service::{ServiceDeps, Services};
use yanto_core::{Config, Db};

fn yanto(config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yanto"))
        .args(args)
        .arg("--config")
        .arg(config)
        .env_remove("YANTO_CONFIG")
        .output()
        .expect("run yanto")
}

fn write_config(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        format!(
            "config_version = 1\n[paths]\ndata_dir = \"{}\"\n",
            dir.join("data").display()
        ),
    )
    .unwrap();
    path
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).trim().to_owned()
}

#[tokio::test]
async fn setup_code_then_owner_reset() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());

    // owner-reset has nothing to reset yet.
    let early = yanto(&cfg, &["owner-reset"]);
    assert!(!early.status.success());
    assert!(String::from_utf8_lossy(&early.stderr).contains("setup-code"));

    let first = yanto(&cfg, &["setup-code"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let code = stdout(&first);
    assert_eq!(code.len(), 19, "XXXX-XXXX-XXXX-XXXX");
    assert!(String::from_utf8_lossy(&first.stderr).contains("live credential"));
    assert!(
        dir.path().join("data/keys/app.key").exists(),
        "app.key is generated"
    );
    let second = stdout(&yanto(&cfg, &["setup-code"]));
    assert_ne!(code, second, "a new code replaces the old one");

    // Create the Owner with the code through the service layer (the dashboard does the same).
    let config = Config::load(&cfg).unwrap();
    let db = Db::open(&config.db_path()).await.unwrap();
    let services = Services::new(ServiceDeps {
        db: db.clone(),
        config: &config,
        clock: Arc::new(SystemClock),
        secrets: Arc::new(Secrets::ephemeral()),
        passwords: Passwords::cheap_for_tests(),
        docker: Arc::new(yanto_core::testing::docker::FakeDocker::new()),
        host_stats: Arc::new(yanto_core::testing::host_stats::FakeHostStats::new()),
        host_mgmt: yanto_core::ports::HostMgmtPorts::unsupported(),
        ingress: yanto_host::ingress_ports_without_registry(
            &config,
            Arc::new(yanto_host::TokioCommands),
            None,
        ),
        host: yanto_core::deploy::HostPorts {
            command: Arc::new(yanto_host::TokioCommands),
            probe: Arc::new(yanto_host::ProcProbe),
            registry_auth: Arc::new(yanto_host::RunDirRegistryAuth::new(dir.path().join("run"))),
        },
    });
    let origin = Origin::new(Channel::Dashboard, None);
    assert!(
        services
            .auth
            .owner_setup(&origin, &code, "boss", "a-long-password-1", None)
            .await
            .is_err(),
        "replaced code is dead"
    );
    services
        .auth
        .owner_setup(&origin, &second, "boss", "a-long-password-1", None)
        .await
        .unwrap();
    db.close().await;

    // setup-code is refused once an Owner exists.
    let refused = yanto(&cfg, &["setup-code"]);
    assert!(!refused.status.success());

    let reset = yanto(&cfg, &["owner-reset"]);
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let url = stdout(&reset);
    assert!(
        url.starts_with("http://127.0.0.1:9197/account/setup#token="),
        "{url}"
    );
    assert!(String::from_utf8_lossy(&reset.stderr).contains("signed out everywhere"));
}
