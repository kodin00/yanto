//! Composition root shared by every subcommand that needs the services: load config,
//! open the database, load (or create) `keys/app.key`, wire `Services`.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use yanto_core::deploy::HostPorts;
use yanto_core::password::Passwords;
use yanto_core::ports::SystemClock;
use yanto_core::secrets::Secrets;
use yanto_core::service::{ServiceDeps, Services};
use yanto_core::{Config, Db};
use yanto_host::{ProcProbe, RunDirRegistryAuth, TokioCommands};

pub struct Runtime {
    pub config: Config,
    pub db: Db,
    pub services: Services,
    pub registry_auth: Arc<RunDirRegistryAuth>,
}

/// systemd's `RuntimeDirectory` (tmpfs) when present, else a directory under the data dir.
fn run_dir(config: &Config) -> std::path::PathBuf {
    let systemd = Path::new("/run/yanto");
    if systemd.is_dir() {
        systemd.to_path_buf()
    } else {
        config.paths.data_dir.join("run")
    }
}

pub fn load_config(path: &Path) -> anyhow::Result<Config> {
    let mut config = Config::load(path).with_context(|| {
        format!(
            "loading config (use --config or YANTO_CONFIG, default {})",
            yanto_core::config::DEFAULT_CONFIG_PATH
        )
    })?;
    config.apply_env(|k| std::env::var(k).ok())?;
    Ok(config)
}

/// Open the database and build the services. New host-side adapters (`yanto-host`) are wired in here.
pub async fn open(config: Config) -> anyhow::Result<Runtime> {
    let db = Db::open(&config.db_path())
        .await
        .context("opening database")?;
    let secrets = Secrets::load_or_create(&config.paths.data_dir).context("loading app.key")?;
    let registry_auth = Arc::new(RunDirRegistryAuth::new(run_dir(&config)));
    let services = Services::new(ServiceDeps {
        db: db.clone(),
        config: &config,
        clock: Arc::new(SystemClock),
        secrets: Arc::new(secrets),
        passwords: Passwords::default(),
        host: HostPorts {
            command: Arc::new(TokioCommands),
            probe: Arc::new(ProcProbe),
            registry_auth: registry_auth.clone(),
        },
    });
    Ok(Runtime {
        config,
        db,
        services,
        registry_auth,
    })
}

/// Run an async subcommand on a fresh runtime.
pub fn block_on<F: std::future::Future<Output = anyhow::Result<()>>>(f: F) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?
        .block_on(f)
}
