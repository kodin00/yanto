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

    // Backups cut short by a crash or restart become `failed` and their partial files go (04 7.8).
    match rt.services.backups.recover_interrupted().await {
        Ok(n) if n > 0 => tracing::warn!(interrupted = n, "marked interrupted backups as failed"),
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "backup recovery failed"),
    }

    // A registry left read-only by an interrupted garbage collection goes back to normal (04 4.3).
    let registry = rt.services.registry.clone();
    tokio::spawn(async move {
        match registry.recover().await {
            Ok(true) => tracing::warn!(
                "restored the Hosted Registry after an interrupted garbage collection"
            ),
            Ok(false) => {}
            Err(err) => tracing::warn!(error = %err, "Hosted Registry recovery failed"),
        }
    });

    let purge = tokio::spawn(purge_loop(rt.services.clone()));
    // Stale per-command registry auth directories from a crashed process.
    rt.registry_auth.sweep();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(yanto_core::deploy::worker::run(
        rt.services.engine(),
        stop.clone(),
    ));
    let state = AppState::load(rt.db.clone(), rt.config.clone(), rt.services.clone()).await;
    // The signal stops the worker from claiming new Deployments right away; it does not wait
    // for the HTTP side, whose SSE/WebSocket streams never end on their own.
    let signalled = stop.clone();
    let shutdown = async move {
        shutdown_signal().await;
        tracing::info!("shutting down");
        signalled.cancel();
    };
    let server = yanto_server::serve(listener, state, shutdown);
    bounded_drain(server, &stop, HTTP_DRAIN).await?;

    // Drain running deployments (30 s, then cancel) before closing the database.
    let _ = worker.await;
    purge.abort();
    rt.db.close().await;
    Ok(())
}

/// How long open HTTP connections (log streams, exec terminals) may linger after the signal.
const HTTP_DRAIN: Duration = Duration::from_secs(5);

/// Run `server` to completion, but at most `grace` past `stop` tripping: axum's graceful
/// shutdown waits for every connection, and a streaming one would hold it until systemd's
/// SIGKILL, skipping the Deployment drain.
async fn bounded_drain(
    server: impl std::future::Future<Output = std::io::Result<()>>,
    stop: &CancellationToken,
    grace: Duration,
) -> std::io::Result<()> {
    tokio::pin!(server);
    tokio::select! {
        r = &mut server => r,
        () = async {
            stop.cancelled().await;
            tokio::time::sleep(grace).await;
        } => {
            tracing::warn!("closing HTTP connections still open after the drain period");
            Ok(())
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_endless_connection_cannot_hold_shutdown() {
        let stop = CancellationToken::new();
        stop.cancel();
        let started = std::time::Instant::now();
        bounded_drain(std::future::pending(), &stop, Duration::from_millis(50))
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_finished_server_returns_its_result() {
        let stop = CancellationToken::new();
        let r = bounded_drain(
            async { Err(std::io::Error::other("bind")) },
            &stop,
            Duration::from_secs(60),
        )
        .await;
        assert!(r.is_err());
    }
}
