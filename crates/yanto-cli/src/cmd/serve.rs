use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use tokio::net::TcpListener;
use yanto_core::ports::CancellationToken;
use yanto_core::service::Services;
use yanto_server::AppState;

use super::common::{block_on, load_config, open};

pub fn run(config_path: &Path) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    crate::logging::init(&config.log)?;
    block_on(serve(config))
}

async fn serve(config: yanto_core::Config) -> anyhow::Result<()> {
    let rt = open(config).await?;
    let addr = format!("{}:{}", rt.config.server.bind, rt.config.server.port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, data_dir = %rt.config.paths.data_dir.display(), "yanto listening");

    if rt.services.auth.setup_pending().await? {
        tracing::warn!("no Owner exists yet: run `yanto setup-code` and open /setup to create one");
    }

    let purge = tokio::spawn(purge_loop(rt.services.clone()));
    // Stale per-command registry auth directories from a crashed process.
    rt.registry_auth.sweep();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(yanto_core::deploy::worker::run(
        rt.services.engine(),
        stop.clone(),
    ));
    let state = AppState::load(rt.db.clone(), rt.config.clone(), rt.services.clone()).await;
    yanto_server::serve(listener, state, shutdown_signal()).await?;

    tracing::info!("shutting down");
    stop.cancel();
    // Drain running deployments (30 s, then cancel) before closing the database.
    let _ = worker.await;
    purge.abort();
    rt.db.close().await;
    Ok(())
}

/// Hourly cleanup of expired sessions and invite/reset tokens (docs/rewrite/01 section 6.3).
async fn purge_loop(services: Services) {
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        match services.auth.purge_expired().await {
            Ok(n) if n > 0 => tracing::info!(removed = n, "purged expired sessions and tokens"),
            Ok(_) => {}
            Err(err) => tracing::warn!(error = %err, "expired-row purge failed"),
        }
    }
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
