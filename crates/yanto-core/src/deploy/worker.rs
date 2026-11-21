//! The queue worker (01 6.1): recovers interrupted Deployments at startup, then claims
//! queued rows (one running per Project, `deploy.max_concurrent` overall) and runs them.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::engine::Engine;
use super::runner;
use crate::actor::Channel;
use crate::db::repo::db_err;
use crate::db::repo::deployments::{self, LogChunk};
use crate::error::CoreError;
use crate::ports::CancellationToken;
use crate::service::audit::{self, AuditActor, AuditEvent};

const POLL: Duration = Duration::from_secs(2);
const DRAIN: Duration = Duration::from_secs(30);
const KILL_GRACE: Duration = Duration::from_secs(6);

/// `running` rows left by a previous process become `interrupted` with a system log line.
pub async fn recover(eng: &Engine) -> Result<usize, CoreError> {
    let mut tx = eng.ctx.db.begin_write().await?;
    let now = eng.ctx.now();
    let ids = deployments::interrupt_running(&mut tx, now).await?;
    for id in &ids {
        let seq = deployments::max_seq(&mut tx, id).await? + 1;
        deployments::append_log(
            &mut tx,
            id,
            &LogChunk {
                seq,
                ts: now,
                stream: "system".into(),
                data: "Yanto restarted during this Deployment.\n".into(),
            },
        )
        .await?;
        if let Some(d) = deployments::get(&mut tx, id).await? {
            let actor = AuditActor {
                actor_type: "system",
                actor_id: None,
                label: "system".into(),
                channel: Channel::System,
                ip: None,
                request_id: None,
            };
            audit::write(
                &eng.ctx,
                &mut tx,
                &actor,
                &AuditEvent::new("deployment.finish", "deployment")
                    .entity(id)
                    .project(&d.project_id)
                    .meta(json!({ "status": "interrupted", "trigger": d.trigger })),
            )
            .await?;
        }
    }
    tx.commit().await.map_err(db_err)?;
    Ok(ids.len())
}

/// Run until `shutdown` trips, then drain: wait up to 30 s for running Deployments,
/// cancel what is left, and wait for the processes to die.
pub async fn run(eng: Arc<Engine>, shutdown: CancellationToken) {
    match recover(&eng).await {
        Ok(n) if n > 0 => tracing::warn!(count = n, "marked running deployments interrupted"),
        Ok(_) => {}
        Err(err) => tracing::error!(error = %err, "deployment recovery failed"),
    }
    let permits = Arc::new(Semaphore::new(eng.max_concurrent));
    let mut tasks: JoinSet<()> = JoinSet::new();
    loop {
        while let Ok(permit) = permits.clone().try_acquire_owned() {
            if shutdown.is_cancelled() {
                break;
            }
            match claim(&eng).await {
                Ok(Some(dep)) => {
                    let token = eng.register_cancel(&dep.id);
                    let e = eng.clone();
                    tasks.spawn(async move {
                        runner::execute(e, dep, token).await;
                        drop(permit);
                    });
                }
                Ok(None) => break,
                Err(err) => {
                    tracing::error!(error = %err, "claiming a queued deployment failed");
                    break;
                }
            }
        }
        tokio::select! {
            () = shutdown.cancelled() => break,
            () = eng.wake.notified() => {}
            () = tokio::time::sleep(POLL) => {}
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    if tasks.is_empty() {
        return;
    }
    tracing::info!(
        running = tasks.len(),
        "waiting for running deployments to finish"
    );
    if tokio::time::timeout(DRAIN, async { while tasks.join_next().await.is_some() {} })
        .await
        .is_err()
    {
        tracing::warn!("cancelling deployments still running after the drain period");
        eng.cancel_all();
        let _ = tokio::time::timeout(KILL_GRACE, async {
            while tasks.join_next().await.is_some() {}
        })
        .await;
    }
}

async fn claim(eng: &Engine) -> Result<Option<deployments::DeploymentRow>, CoreError> {
    let mut tx = eng.ctx.db.begin_write().await?;
    let row = deployments::claim_next(&mut tx, eng.ctx.now()).await?;
    tx.commit().await.map_err(db_err)?;
    Ok(row)
}

/// Used by tests and the CLI to run one claim cycle without the loop.
pub async fn run_once(eng: Arc<Engine>) -> bool {
    match claim(&eng).await {
        Ok(Some(dep)) => {
            let token = eng.register_cancel(&dep.id);
            runner::execute(eng.clone(), dep, token).await;
            true
        }
        _ => false,
    }
}
