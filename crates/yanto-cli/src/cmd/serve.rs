use std::path::Path;

use anyhow::Context;
use tokio::net::TcpListener;
use yanto_core::{Config, Db};
use yanto_server::AppState;

pub fn run(config_path: &Path) -> anyhow::Result<()> {
    let mut config = Config::load(config_path).with_context(|| {
        format!(
            "loading config (use --config or YANTO_CONFIG, default {})",
            yanto_core::config::DEFAULT_CONFIG_PATH
        )
    })?;
    config.apply_env(|k| std::env::var(k).ok())?;
    crate::logging::init(&config.log)?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?
        .block_on(serve(config))
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let db = Db::open(&config.db_path())
        .await
        .context("opening database")?;
    let addr = format!("{}:{}", config.server.bind, config.server.port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, data_dir = %config.paths.data_dir.display(), "yanto listening");

    let state = AppState::new(db.clone(), config);
    yanto_server::serve(listener, state, shutdown_signal()).await?;

    tracing::info!("shutting down");
    db.close().await;
    Ok(())
}

/// Resolves on SIGINT or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(%err, "cannot listen for SIGINT");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(err) => {
                tracing::error!(%err, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("received SIGINT"),
        () = term => tracing::info!("received SIGTERM"),
    }
}
