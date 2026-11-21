//! Composition root shared by every subcommand that needs the services: load config,
//! open the database, load (or create) `keys/app.key`, wire `Services`.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use yanto_core::password::Passwords;
use yanto_core::ports::{HostMgmtPorts, SystemClock, UnavailableProjectBackups};
use yanto_core::secrets::Secrets;
use yanto_core::service::{ServiceDeps, Services};
use yanto_core::{Config, Db};

pub struct Runtime {
    pub config: Config,
    pub db: Db,
    pub services: Services,
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
    let services = Services::new(ServiceDeps {
        db: db.clone(),
        config: &config,
        clock: Arc::new(SystemClock),
        secrets: Arc::new(secrets),
        passwords: Passwords::default(),
        host_mgmt: host_ports(&config),
    });
    Ok(Runtime {
        config,
        db,
        services,
    })
}

/// The real Host ports for cron, `authorized_keys`, users and backups (M5).
pub fn host_ports(config: &Config) -> HostMgmtPorts {
    use yanto_host::{HostAuthorizedKeys, HostBackup, HostCron, HostCronExec, SystemUsers};
    let h = &config.host;
    // The cron file must be owned by whoever Yanto runs as: root in production (so cron accepts
    // it), the current user in development and tests.
    let owner = (
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw(),
    );
    let users = Arc::new(SystemUsers::new(&h.passwd_file));
    HostMgmtPorts {
        users: users.clone(),
        cron: Arc::new(HostCron::new(&h.etc_dir, &h.spool_dir, owner)),
        cron_exec: Arc::new(HostCronExec::new(&h.run_dir)),
        authorized_keys: Arc::new(HostAuthorizedKeys::new(users)),
        backup_host: Arc::new(HostBackup::new(&h.run_dir)),
        // TODO(M2-merge): the Docker-backed Project backup port replaces this.
        project_backups: Arc::new(UnavailableProjectBackups),
    }
}

/// Run an async subcommand on a fresh runtime.
pub fn block_on<T, F: std::future::Future<Output = anyhow::Result<T>>>(f: F) -> anyhow::Result<T> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?
        .block_on(f)
}
