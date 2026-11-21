//! Deployments: enqueue (manual, API, Deploy Token, GitHub), cancel, rollback with
//! preview, stored logs, SSE log and status streams (docs/rewrite/04 1.1, 1.4, 1.5; 03 a.6).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use super::projects::triggered_by;
use crate::actor::{Access, Actor, Channel, Need, OpSpec, Origin, Permission, Scope};
use crate::crypto::{ct_eq, new_id, sha256_hex};
use crate::db::repo::deployments::{self, DeploymentFilter, DeploymentRow, LogChunk};
use crate::db::repo::{db_err, projects};
use crate::deploy::tail::TailSink;
use crate::deploy::{Engine, HubEvent, env, files, validate};
use crate::error::CoreError;
use crate::ports::{CancellationToken, CommandSpec, LogSink};
use crate::ratelimit::FailureLimiter;
use crate::secrets::Secret;

pub const OP_PROJECT_DEPLOY: OpSpec = OpSpec::new(
    "project_deploy",
    Access::Project(Need::Permission(Permission::Deploy)),
    Scope::Write,
);
pub const OP_PROJECT_ROLLBACK_PREVIEW: OpSpec = OpSpec::new(
    "project_rollback_preview",
    Access::Project(Need::Permission(Permission::Deploy)),
    Scope::Read,
);
pub const OP_PROJECT_ROLLBACK: OpSpec = OpSpec::new(
    "project_rollback",
    Access::Project(Need::Permission(Permission::Deploy)),
    Scope::Admin,
);
/// `git reset --hard origin/<branch>` on the Project's checkout (04 1.6): discards local commits
/// and changes on the Host, so it is a destructive (admin scope) deploy-permission action.
pub const OP_PROJECT_RESET_CHECKOUT: OpSpec = OpSpec::new(
    "project_reset_checkout",
    Access::Project(Need::Permission(Permission::Deploy)),
    Scope::Admin,
);
pub const OP_DEPLOYMENTS_LIST: OpSpec =
    OpSpec::new("deployments_list", Access::Authenticated, Scope::Read);
pub const OP_DEPLOYMENT_GET: OpSpec =
    OpSpec::new("deployment_get", Access::Project(Need::View), Scope::Read);
pub const OP_DEPLOYMENT_LOGS: OpSpec =
    OpSpec::new("deployment_logs", Access::Project(Need::View), Scope::Read);
pub const OP_DEPLOYMENT_LOGS_STREAM: OpSpec = OpSpec::new(
    "deployment_logs_stream",
    Access::Project(Need::View),
    Scope::Read,
);
pub const OP_DEPLOYMENT_CANCEL: OpSpec = OpSpec::new(
    "deployment_cancel",
    Access::Project(Need::Permission(Permission::Deploy)),
    Scope::Write,
);
pub const OP_DEPLOYMENTS_EVENTS_STREAM: OpSpec = OpSpec::new(
    "deployments_events_stream",
    Access::Authenticated,
    Scope::Read,
);
/// `POST /deploy`: validates its own bearer Deploy Token, so no actor is needed.
pub const OP_DEPLOY_WEBHOOK: OpSpec = OpSpec::new("deploy_webhook", Access::Public, Scope::Read);
/// `POST /webhooks/github`: validates its own HMAC signature.
pub const OP_GITHUB_WEBHOOK: OpSpec = OpSpec::new("github_webhook", Access::Public, Scope::Read);

/// Sub-check, not an operation: a deploy that carries an env payload also needs `secrets`.
const ENV_PAYLOAD_CHECK: OpSpec = OpSpec::new(
    "project_deploy_env_payload",
    Access::Project(Need::Permission(Permission::Secrets)),
    Scope::Write,
);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_PROJECT_DEPLOY,
    &OP_PROJECT_ROLLBACK_PREVIEW,
    &OP_PROJECT_ROLLBACK,
    &OP_PROJECT_RESET_CHECKOUT,
    &OP_DEPLOYMENTS_LIST,
    &OP_DEPLOYMENT_GET,
    &OP_DEPLOYMENT_LOGS,
    &OP_DEPLOYMENT_LOGS_STREAM,
    &OP_DEPLOYMENT_CANCEL,
    &OP_DEPLOYMENTS_EVENTS_STREAM,
    &OP_DEPLOY_WEBHOOK,
    &OP_GITHUB_WEBHOOK,
];

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 500;
const GITHUB_BODY_CAP: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct DeploymentDto {
    pub id: String,
    pub project_id: String,
    pub status: String,
    pub trigger: String,
    pub triggered_by: Option<String>,
    pub target_ref: Option<String>,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    pub rollback_from_deployment_id: Option<String>,
    pub image_digest: Option<String>,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    pub log_bytes: i64,
    pub log_truncated: bool,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// Failed at the fast-forward pull: the UI offers "Reset checkout to origin/<branch>".
    pub reset_offered: bool,
}

impl From<DeploymentRow> for DeploymentDto {
    fn from(d: DeploymentRow) -> Self {
        let reset_offered = d.status == "failed"
            && d.error
                .as_deref()
                .is_some_and(|e| e.contains(crate::deploy::runner::PULL_STEP));
        Self {
            id: d.id,
            project_id: d.project_id,
            status: d.status,
            trigger: d.trigger,
            triggered_by: d.triggered_by,
            target_ref: d.target_ref,
            commit_sha: d.commit_sha,
            commit_message: d.commit_message,
            rollback_from_deployment_id: d.rollback_from_deployment_id,
            image_digest: d.image_digest,
            exit_code: d.exit_code,
            error: d.error,
            log_bytes: d.log_bytes,
            log_truncated: d.log_truncated,
            queued_at: d.queued_at,
            started_at: d.started_at,
            finished_at: d.finished_at,
            reset_offered,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DeployRequest {
    /// Git Projects: deploy this branch, tag or commit instead of the branch tip.
    pub target_ref: Option<String>,
    /// `.env` text carried by this Deployment (also needs `secrets`).
    pub env: Option<Secret>,
}

/// The result of [`DeploymentService::reset_checkout`].
#[derive(Debug, Clone, Serialize)]
pub struct ResetOutcome {
    pub branch: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Enqueued {
    pub deployment: DeploymentDto,
    pub reused: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DeploymentQuery {
    pub project_id: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct RollbackRequest {
    /// A prior successful Deployment whose code/image and config to restore.
    pub deployment_id: Option<String>,
    /// Git Projects only: an explicit ref (no config snapshot is restored for this form).
    pub git_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Revision {
    pub deployment_id: Option<String>,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    pub image_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileStat {
    pub path: String,
    pub added: String,
    pub removed: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffLine {
    /// `+`, `-` or a space.
    pub kind: char,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EnvDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RollbackPreview {
    pub current: Option<Revision>,
    pub target: Revision,
    pub restores_config: bool,
    pub commits_behind: Option<u32>,
    pub commits_ahead: Option<u32>,
    pub files: Vec<FileStat>,
    pub files_truncated: bool,
    pub compose_diff: Vec<DiffLine>,
    pub env_diff: EnvDiff,
    pub image_available: Option<bool>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum LogStreamItem {
    Log(LogChunk),
    Status(String),
    Done(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusEvent {
    pub deployment_id: String,
    pub project_id: String,
    pub status: String,
    pub done: bool,
}

/// Outcome of a GitHub delivery.
#[derive(Debug, Clone)]
pub enum GithubOutcome {
    Pong,
    Duplicate,
    Ignored(&'static str),
    Queued(Box<Enqueued>),
}

pub struct GithubDelivery<'a> {
    pub signature: Option<&'a str>,
    pub event: Option<&'a str>,
    pub delivery_id: Option<&'a str>,
    pub form_encoded: bool,
    pub body: &'a [u8],
}

struct Hooks {
    deploy: FailureLimiter,
    github: FailureLimiter,
    seen: Mutex<VecDeque<String>>,
}

#[derive(Clone)]
pub struct DeploymentService {
    ctx: Ctx,
    eng: Arc<Engine>,
    hooks: Arc<Hooks>,
}

impl std::fmt::Debug for DeploymentService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeploymentService")
    }
}

struct NewDeployment {
    project_id: String,
    trigger: &'static str,
    triggered_by: String,
    target_ref: Option<String>,
    pending_env: Option<Secret>,
    rollback: Option<RollbackSeed>,
    audit_action: &'static str,
    meta: Value,
}

struct RollbackSeed {
    from_id: String,
    compose: Option<String>,
    env: Option<Secret>,
}

fn not_found() -> CoreError {
    CoreError::not_found("deployment not found")
}

impl DeploymentService {
    pub(super) fn new(ctx: Ctx, eng: Arc<Engine>) -> Self {
        Self {
            ctx,
            eng,
            hooks: Arc::new(Hooks {
                deploy: FailureLimiter::new(10, Duration::from_secs(15 * 60)),
                github: FailureLimiter::new(60, Duration::from_secs(60)),
                seen: Mutex::new(VecDeque::new()),
            }),
        }
    }

    async fn enqueue(
        &self,
        who: &AuditActor,
        new: NewDeployment,
    ) -> Result<(DeploymentRow, bool), CoreError> {
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        projects::get(&mut tx, &new.project_id)
            .await?
            .ok_or_else(|| CoreError::not_found("project not found"))?;
        if let Some(q) = deployments::queued_for(&mut tx, &new.project_id).await? {
            // Only a plain trigger may coalesce into the queued row (04 1.3); a ref, an env
            // payload or a rollback would otherwise be accepted and then silently dropped.
            if new.rollback.is_some()
                || new.target_ref.is_some()
                || new.pending_env.is_some()
                || new.trigger == "rollback"
            {
                return Err(CoreError::conflict(
                    "a deployment is already queued for this project; cancel it first",
                ));
            }
            audit::write(
                &self.ctx,
                &mut tx,
                who,
                &AuditEvent::new(new.audit_action, "deployment")
                    .entity(&q.id)
                    .project(&new.project_id)
                    .meta(json!({ "trigger": new.trigger, "reused": true })),
            )
            .await?;
            tx.commit().await.map_err(db_err)?;
            return Ok((q, true));
        }
        let id = new_id("dep");
        let seal = |col: &str, s: &Secret| self.ctx.secrets.seal("deployments", col, &id, s);
        let (compose, env_snapshot, from) = match &new.rollback {
            Some(r) => (
                r.compose.clone(),
                r.env
                    .as_ref()
                    .map(|e| seal("env_snapshot_enc", e))
                    .transpose()?,
                Some(r.from_id.clone()),
            ),
            None => (None, None, None),
        };
        let row = DeploymentRow {
            id: id.clone(),
            project_id: new.project_id.clone(),
            status: "queued".into(),
            trigger: new.trigger.into(),
            triggered_by: Some(new.triggered_by.clone()),
            target_ref: new.target_ref.clone(),
            commit_sha: None,
            commit_message: None,
            rollback_from_deployment_id: from,
            image_digest: None,
            compose_snapshot: compose,
            env_snapshot_enc: env_snapshot,
            pending_env_enc: new
                .pending_env
                .as_ref()
                .map(|e| seal("pending_env_enc", e))
                .transpose()?,
            exit_code: None,
            error: None,
            log_bytes: 0,
            log_truncated: false,
            queued_at: now,
            started_at: None,
            finished_at: None,
        };
        deployments::insert(&mut tx, &row).await?;
        let mut meta = new.meta.clone();
        meta["trigger"] = json!(new.trigger);
        meta["reused"] = json!(false);
        audit::write(
            &self.ctx,
            &mut tx,
            who,
            &AuditEvent::new(new.audit_action, "deployment")
                .entity(&id)
                .project(&new.project_id)
                .meta(meta),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        self.eng.hub.publish(HubEvent::Status {
            deployment_id: id,
            project_id: new.project_id,
            status: "queued".into(),
        });
        self.eng.wake_worker();
        Ok((row, false))
    }

    /// Enqueue a Deployment for the Project (`202 {deployment, reused}`).
    pub async fn deploy(
        &self,
        actor: &Actor,
        project_id: &str,
        req: &DeployRequest,
    ) -> Result<Enqueued, CoreError> {
        actor.authorize(&OP_PROJECT_DEPLOY, Some(project_id))?;
        if req.env.is_some() {
            actor.authorize(&ENV_PAYLOAD_CHECK, Some(project_id))?;
        }
        let target_ref = req
            .target_ref
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|r| validate::git_ref("target_ref", r))
            .transpose()?;
        if let Some(env_text) = &req.env {
            if env_text.expose().len() > 1024 * 1024 {
                return Err(CoreError::validation("env", "must be at most 1 MiB"));
            }
            let parsed = env::parse(env_text.expose());
            if !parsed.invalid_lines.is_empty() {
                return Err(CoreError::validation("env", "lines must be KEY=VALUE"));
            }
        }
        if target_ref.is_some() {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            let p = projects::get(&mut conn, project_id)
                .await?
                .ok_or_else(|| CoreError::not_found("project not found"))?;
            if !p.is_git() {
                return Err(CoreError::validation(
                    "target_ref",
                    "only Git projects can deploy a ref",
                ));
            }
        }
        let trigger = match actor.channel {
            Channel::Rest | Channel::Mcp => "api",
            _ => "manual",
        };
        let (row, reused) = self
            .enqueue(
                &AuditActor::from_actor(actor),
                NewDeployment {
                    project_id: project_id.to_owned(),
                    trigger,
                    triggered_by: triggered_by(actor),
                    target_ref,
                    pending_env: req.env.clone(),
                    rollback: None,
                    audit_action: "deployment.trigger",
                    meta: json!({ "with_env": req.env.is_some() }),
                },
            )
            .await?;
        Ok(Enqueued {
            deployment: row.into(),
            reused,
        })
    }

    pub async fn list(
        &self,
        actor: &Actor,
        q: &DeploymentQuery,
    ) -> Result<Vec<DeploymentDto>, CoreError> {
        actor.authorize(&OP_DEPLOYMENTS_LIST, None)?;
        let visible = actor
            .visible_projects()
            .map(|v| v.into_iter().map(str::to_owned).collect::<Vec<_>>());
        if let (Some(v), Some(p)) = (&visible, &q.project_id)
            && !v.contains(p)
        {
            return Err(CoreError::not_found("project not found"));
        }
        let status = match &q.status {
            Some(s)
                if !matches!(
                    s.as_str(),
                    "queued" | "running" | "success" | "failed" | "cancelled" | "interrupted"
                ) =>
            {
                return Err(CoreError::validation("status", "unknown status"));
            }
            other => other.clone(),
        };
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let rows = deployments::list(
            &mut conn,
            &DeploymentFilter {
                visible_projects: visible,
                project_id: q.project_id.clone(),
                status,
                limit: q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
            },
        )
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn find(&self, id: &str) -> Result<DeploymentRow, CoreError> {
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        deployments::get(&mut conn, id).await?.ok_or_else(not_found)
    }

    /// Load then authorize; unknown ids and Deployments of invisible Projects are both 404.
    async fn find_for(
        &self,
        actor: &Actor,
        op: &OpSpec,
        id: &str,
    ) -> Result<DeploymentRow, CoreError> {
        let Some(row) = ({
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            deployments::get(&mut conn, id).await?
        }) else {
            // Unknown id: scope and role are still checked first, and a Member gets the same
            // answer (404) as for a Deployment of a Project they cannot see.
            actor.authorize(op, Some("-"))?;
            return Err(not_found());
        };
        actor.authorize(op, Some(&row.project_id))?;
        Ok(row)
    }

    pub async fn get(&self, actor: &Actor, id: &str) -> Result<DeploymentDto, CoreError> {
        Ok(self.find_for(actor, &OP_DEPLOYMENT_GET, id).await?.into())
    }

    /// Stored log text, optionally only the last `tail` lines.
    pub async fn logs(
        &self,
        actor: &Actor,
        id: &str,
        tail: Option<usize>,
    ) -> Result<String, CoreError> {
        self.find_for(actor, &OP_DEPLOYMENT_LOGS, id).await?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let mut text = String::new();
        let mut last = 0;
        loop {
            let chunks = deployments::logs_after(&mut conn, id, last, 500).await?;
            let Some(end) = chunks.last().map(|c| c.seq) else {
                break;
            };
            for c in &chunks {
                text.push_str(&c.data);
            }
            last = end;
        }
        if let Some(n) = tail {
            let lines: Vec<&str> = text.lines().collect();
            let skip = lines.len().saturating_sub(n);
            text = lines[skip..].join("\n");
            if !text.is_empty() {
                text.push('\n');
            }
        }
        Ok(text)
    }

    /// Live log: replay from `last_event_id` (exclusive), then follow, then `Done`.
    pub async fn log_stream(
        &self,
        actor: &Actor,
        id: &str,
        last_event_id: Option<i64>,
    ) -> Result<ReceiverStream<LogStreamItem>, CoreError> {
        self.find_for(actor, &OP_DEPLOYMENT_LOGS_STREAM, id).await?;
        // Subscribe first, then replay, so nothing falls between the two.
        let mut rx = self.eng.hub.subscribe();
        let (tx, out) = mpsc::channel(256);
        let ctx = self.ctx.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let mut last = last_event_id.unwrap_or(0);
            let replay = async |last: &mut i64| -> bool {
                let Ok(mut conn) = ctx.db.reader().acquire().await else {
                    return false;
                };
                loop {
                    let Ok(chunks) = deployments::logs_after(&mut conn, &id, *last, 500).await
                    else {
                        return false;
                    };
                    if chunks.is_empty() {
                        return true;
                    }
                    for c in chunks {
                        *last = c.seq;
                        if tx.send(LogStreamItem::Log(c)).await.is_err() {
                            return false;
                        }
                    }
                }
            };
            // Read the status before replaying. A terminal status is only written after the
            // last log chunk is committed, so replaying after it sees every chunk; reading it
            // after the replay would let chunks committed in between be skipped.
            let status = match ctx.db.reader().acquire().await {
                Ok(mut c) => deployments::get(&mut c, &id)
                    .await
                    .ok()
                    .flatten()
                    .map(|d| d.status),
                Err(_) => None,
            };
            if !replay(&mut last).await {
                return;
            }
            match status.as_deref() {
                Some("queued" | "running") => {
                    let _ = tx
                        .send(LogStreamItem::Status(status.unwrap_or_default()))
                        .await;
                }
                Some(done) => {
                    let _ = tx.send(LogStreamItem::Done(done.to_owned())).await;
                    return;
                }
                None => return,
            }
            loop {
                match rx.recv().await {
                    Ok(ev) if ev.deployment_id() == id => match ev {
                        HubEvent::Log { chunk, .. } => {
                            if chunk.seq > last {
                                last = chunk.seq;
                                if tx.send(LogStreamItem::Log(chunk)).await.is_err() {
                                    return;
                                }
                            }
                        }
                        HubEvent::Status { status, .. } => {
                            if tx.send(LogStreamItem::Status(status)).await.is_err() {
                                return;
                            }
                        }
                        HubEvent::Done { status, .. } => {
                            if replay(&mut last).await {
                                let _ = tx.send(LogStreamItem::Done(status)).await;
                            }
                            return;
                        }
                    },
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if !replay(&mut last).await {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        Ok(ReceiverStream::new(out))
    }

    /// Status changes of the Deployments the actor may see (Projects as of stream start).
    pub async fn events_stream(
        &self,
        actor: &Actor,
    ) -> Result<ReceiverStream<StatusEvent>, CoreError> {
        actor.authorize(&OP_DEPLOYMENTS_EVENTS_STREAM, None)?;
        let visible: Option<BTreeSet<String>> = actor
            .visible_projects()
            .map(|v| v.into_iter().map(str::to_owned).collect());
        let mut rx = self.eng.hub.subscribe();
        let (tx, out) = mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        if visible
                            .as_ref()
                            .is_some_and(|v| !v.contains(ev.project_id()))
                        {
                            continue;
                        }
                        let item = match ev {
                            HubEvent::Status {
                                deployment_id,
                                project_id,
                                status,
                            } => StatusEvent {
                                deployment_id,
                                project_id,
                                status,
                                done: false,
                            },
                            HubEvent::Done {
                                deployment_id,
                                project_id,
                                status,
                            } => StatusEvent {
                                deployment_id,
                                project_id,
                                status,
                                done: true,
                            },
                            HubEvent::Log { .. } => continue,
                        };
                        if tx.send(item).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        Ok(ReceiverStream::new(out))
    }

    /// Cancel a queued (immediately) or running (SIGTERM, then SIGKILL) Deployment.
    pub async fn cancel(&self, actor: &Actor, id: &str) -> Result<DeploymentDto, CoreError> {
        let dep = self.find_for(actor, &OP_DEPLOYMENT_CANCEL, id).await?;
        match dep.status.as_str() {
            "queued" => {
                let mut tx = self.ctx.db.begin_write().await?;
                let changed = deployments::cancel_queued(&mut tx, id, self.ctx.now()).await?;
                if !changed {
                    return Err(CoreError::conflict(
                        "the deployment has just started; cancel it again",
                    ));
                }
                audit::write(
                    &self.ctx,
                    &mut tx,
                    &AuditActor::from_actor(actor),
                    &AuditEvent::new("deployment.cancel", "deployment")
                        .entity(id)
                        .project(&dep.project_id)
                        .meta(json!({ "was": "queued" })),
                )
                .await?;
                tx.commit().await.map_err(db_err)?;
                self.eng.hub.publish(HubEvent::Done {
                    deployment_id: id.to_owned(),
                    project_id: dep.project_id.clone(),
                    status: "cancelled".into(),
                });
            }
            "running" => {
                if !self.eng.cancel(id) {
                    return Err(CoreError::conflict(
                        "the deployment is running in another process and cannot be cancelled from here",
                    ));
                }
                let mut tx = self.ctx.db.begin_write().await?;
                audit::write(
                    &self.ctx,
                    &mut tx,
                    &AuditActor::from_actor(actor),
                    &AuditEvent::new("deployment.cancel", "deployment")
                        .entity(id)
                        .project(&dep.project_id)
                        .meta(json!({ "was": "running" })),
                )
                .await?;
                tx.commit().await.map_err(db_err)?;
            }
            _ => return Err(CoreError::conflict("the deployment has already finished")),
        }
        Ok(self.find(id).await?.into())
    }

    // ---- rollback ----------------------------------------------------------------

    async fn rollback_target(
        &self,
        project_id: &str,
        req: &RollbackRequest,
    ) -> Result<
        (
            Option<DeploymentRow>,
            Option<DeploymentRow>,
            projects::ProjectRow,
        ),
        CoreError,
    > {
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let project = projects::get(&mut conn, project_id)
            .await?
            .ok_or_else(|| CoreError::not_found("project not found"))?;
        let successes = deployments::successes(&mut conn, project_id, 50).await?;
        let current = successes.first().cloned();
        let same = |a: &DeploymentRow, b: &DeploymentRow| {
            if project.is_git() {
                a.commit_sha == b.commit_sha
            } else {
                a.image_digest == b.image_digest
            }
        };
        let target = if let Some(id) = &req.deployment_id {
            let t = deployments::get(&mut conn, id)
                .await?
                .ok_or_else(not_found)?;
            if t.project_id != project_id || t.status != "success" {
                return Err(CoreError::validation(
                    "deployment_id",
                    "pick a successful Deployment of this project",
                ));
            }
            Some(t)
        } else if req.git_ref.is_some() {
            None
        } else {
            match &current {
                Some(c) => successes
                    .iter()
                    .find(|d| !same(c, d) && d.compose_snapshot.is_some())
                    .cloned(),
                None => None,
            }
        };
        if req.git_ref.is_none() && target.is_none() {
            return Err(CoreError::conflict(
                "there is no earlier successful deployment to roll back to",
            ));
        }
        Ok((current, target, project))
    }

    /// Read-only: what a rollback would change (04 1.1 "Rollback").
    pub async fn rollback_preview(
        &self,
        actor: &Actor,
        project_id: &str,
        req: &RollbackRequest,
    ) -> Result<RollbackPreview, CoreError> {
        actor.authorize(&OP_PROJECT_ROLLBACK_PREVIEW, Some(project_id))?;
        let git_ref = req
            .git_ref
            .as_deref()
            .map(|r| validate::git_ref("git_ref", r))
            .transpose()?;
        let (current, target, project) = self.rollback_target(project_id, req).await?;
        let rev = |d: &DeploymentRow| Revision {
            deployment_id: Some(d.id.clone()),
            commit_sha: d.commit_sha.clone(),
            commit_message: d.commit_message.clone(),
            image_digest: d.image_digest.clone(),
        };
        let target_rev = match (&target, &git_ref) {
            (Some(t), _) => rev(t),
            (None, Some(r)) => Revision {
                deployment_id: None,
                commit_sha: Some(r.clone()),
                commit_message: None,
                image_digest: None,
            },
            (None, None) => unreachable!("rollback_target guarantees a target"),
        };
        let mut notes = Vec::new();
        let mut preview = RollbackPreview {
            current: current.as_ref().map(rev),
            target: target_rev.clone(),
            restores_config: target.is_some(),
            commits_behind: None,
            commits_ahead: None,
            files: Vec::new(),
            files_truncated: false,
            compose_diff: Vec::new(),
            env_diff: EnvDiff::default(),
            image_available: None,
            notes: Vec::new(),
        };
        let dir = files::project_dir(&self.eng.data_dir, &project.slug)?;
        if project.is_git() {
            let cur = current.as_ref().and_then(|c| c.commit_sha.clone());
            let tgt = target_rev.commit_sha.clone();
            if let (Some(cur), Some(tgt)) = (cur, tgt) {
                if dir.join(".git").exists() {
                    preview.commits_behind = self.git_count(&dir, &format!("{tgt}..{cur}")).await;
                    preview.commits_ahead = self.git_count(&dir, &format!("{cur}..{tgt}")).await;
                    let (files, truncated) = self.git_numstat(&dir, &cur, &tgt).await;
                    preview.files = files;
                    preview.files_truncated = truncated;
                } else {
                    notes.push("No checkout exists yet, so commit counts and the file diff are unavailable.".into());
                }
            }
        } else if let Some(d) = &target_rev.image_digest {
            preview.image_available = self.image_available(&project, d).await;
            if preview.image_available == Some(false) {
                notes.push(
                    "The target image digest is no longer available from its registry.".into(),
                );
            }
        } else {
            notes.push(
                "The target Deployment recorded no image digest, so the image cannot be pinned."
                    .into(),
            );
        }
        if let (Some(t), Some(c)) = (&target, &current) {
            preview.compose_diff = line_diff(
                c.compose_snapshot.as_deref().unwrap_or(""),
                t.compose_snapshot.as_deref().unwrap_or(""),
            );
            preview.env_diff = self.env_diff(c, t)?;
        }
        preview.notes = notes;
        Ok(preview)
    }

    fn env_diff(&self, cur: &DeploymentRow, tgt: &DeploymentRow) -> Result<EnvDiff, CoreError> {
        let open = |d: &DeploymentRow| -> Result<env::EnvFile, CoreError> {
            Ok(match &d.env_snapshot_enc {
                Some(e) => env::parse(
                    self.ctx
                        .secrets
                        .open("deployments", "env_snapshot_enc", &d.id, e)?
                        .expose(),
                ),
                None => env::EnvFile::default(),
            })
        };
        let (a, b) = (open(cur)?, open(tgt)?);
        let av: BTreeMap<&str, &str> = a.vars().into_iter().collect();
        let bv: BTreeMap<&str, &str> = b.vars().into_iter().collect();
        let mut diff = EnvDiff::default();
        for (k, v) in &bv {
            match av.get(k) {
                None => diff.added.push((*k).to_owned()),
                Some(old) if old != v => diff.changed.push((*k).to_owned()),
                Some(_) => {}
            }
        }
        diff.removed = av
            .keys()
            .filter(|k| !bv.contains_key(*k))
            .map(|k| (*k).to_owned())
            .collect();
        Ok(diff)
    }

    async fn quick(&self, spec: CommandSpec) -> Option<String> {
        let out = self
            .eng
            .host
            .command
            .run(
                spec.capture().timeout(Duration::from_secs(60)),
                Arc::new(TailSink::default()) as Arc<dyn LogSink>,
                &CancellationToken::new(),
            )
            .await
            .ok()?;
        (out.exit_code == 0).then_some(out.stdout)
    }

    async fn git_count(&self, dir: &std::path::Path, range: &str) -> Option<u32> {
        self.quick(CommandSpec::new("git", &["rev-list", "--count", range]).cwd(dir))
            .await?
            .trim()
            .parse()
            .ok()
    }

    async fn git_numstat(
        &self,
        dir: &std::path::Path,
        from: &str,
        to: &str,
    ) -> (Vec<FileStat>, bool) {
        let Some(out) = self
            .quick(CommandSpec::new("git", &["diff", "--numstat", from, to]).cwd(dir))
            .await
        else {
            return (Vec::new(), false);
        };
        let mut files: Vec<FileStat> = out
            .lines()
            .filter_map(|l| {
                let mut p = l.splitn(3, '\t');
                Some(FileStat {
                    added: p.next()?.to_owned(),
                    removed: p.next()?.to_owned(),
                    path: p.next()?.to_owned(),
                })
            })
            .collect();
        let truncated = files.len() > 40;
        files.truncate(40);
        (files, truncated)
    }

    async fn image_available(&self, p: &projects::ProjectRow, digest: &str) -> Option<bool> {
        let _ = p;
        let out = self
            .eng
            .host
            .command
            .run(
                CommandSpec::new("docker", &["manifest", "inspect", digest])
                    .capture()
                    .timeout(Duration::from_secs(60)),
                Arc::new(TailSink::default()) as Arc<dyn LogSink>,
                &CancellationToken::new(),
            )
            .await
            .ok()?;
        Some(out.exit_code == 0)
    }

    /// Enqueue a rollback Deployment restoring the target's code/image and snapshotted config (D25).
    /// `git reset --hard origin/<branch>` after a non-fast-forward pull. Refused while a
    /// Deployment runs and for image Projects. Audited with the revisions before and after.
    pub async fn reset_checkout(
        &self,
        actor: &Actor,
        project_id: &str,
    ) -> Result<ResetOutcome, CoreError> {
        use crate::deploy::runner::{FETCH_TIMEOUT, REV_TIMEOUT, git_env_for, open_deploy_key};
        actor.authorize(&OP_PROJECT_RESET_CHECKOUT, Some(project_id))?;
        let project = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            if deployments::running_for(&mut conn, project_id)
                .await?
                .is_some()
            {
                return Err(CoreError::conflict(
                    "a deployment is running; wait for it to finish",
                ));
            }
            projects::get(&mut conn, project_id)
                .await?
                .ok_or_else(|| CoreError::not_found("project not found"))?
        };
        if !project.is_git() {
            return Err(CoreError::validation(
                "project",
                "only Git projects have a checkout to reset",
            ));
        }
        let branch = project.branch.clone().unwrap_or_else(|| "master".into());
        validate::git_ref("branch", &branch)?;
        let dir = files::project_dir(&self.eng.data_dir, &project.slug)?;
        if !dir.join(".git").exists() {
            return Err(CoreError::conflict(
                "there is no checkout yet; deploy the project first",
            ));
        }
        let key = open_deploy_key(&self.eng, &project)
            .await
            .map_err(|e| e.into_core())?;
        files::ensure_dir(&self.eng.data_dir.join("ssh"), 0o700)?;
        let env = git_env_for(&self.eng.data_dir, key.as_ref());
        let run = |args: &[&str], timeout: Duration, capture: bool| {
            let mut s = CommandSpec::new("git", args)
                .cwd(dir.clone())
                .timeout(timeout);
            s.env.clone_from(&env);
            if capture {
                s = s.capture();
            }
            s
        };
        let tail = Arc::new(TailSink::default());
        let exec = |spec: CommandSpec, what: &'static str| {
            let tail = tail.clone();
            let command = self.eng.host.command.clone();
            async move {
                let out = command
                    .run(
                        spec,
                        tail.clone() as Arc<dyn LogSink>,
                        &CancellationToken::new(),
                    )
                    .await?;
                if !out.success() {
                    return Err(CoreError::conflict(format!(
                        "{what} failed (exit code {}): {}",
                        out.exit_code,
                        tail.text()
                    )));
                }
                Ok(out.stdout.trim().to_owned())
            }
        };
        let before = exec(
            run(&["rev-parse", "HEAD"], REV_TIMEOUT, true),
            "git rev-parse",
        )
        .await
        .unwrap_or_default();
        let outcome = async {
            exec(
                run(&["fetch", "origin", &branch], FETCH_TIMEOUT, false),
                "git fetch",
            )
            .await?;
            let remote = format!("origin/{branch}");
            exec(
                run(&["reset", "--hard", &remote], REV_TIMEOUT, false),
                "git reset",
            )
            .await?;
            exec(
                run(&["rev-parse", "HEAD"], REV_TIMEOUT, true),
                "git rev-parse",
            )
            .await
        }
        .await;
        drop(key);
        let mut tx = self.ctx.db.begin_write().await?;
        let mut ev = AuditEvent::new("project.checkout_reset", "project")
            .entity(project_id)
            .project(project_id);
        let meta = match &outcome {
            Ok(after) => json!({ "branch": branch, "from": before, "to": after }),
            Err(e) => {
                ev.outcome = audit::Outcome::Error;
                json!({ "branch": branch, "from": before, "error": e.message })
            }
        };
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &ev.meta(meta),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        let after = outcome?;
        Ok(ResetOutcome {
            branch,
            from: before,
            to: after,
        })
    }

    pub async fn rollback(
        &self,
        actor: &Actor,
        project_id: &str,
        req: &RollbackRequest,
    ) -> Result<Enqueued, CoreError> {
        actor.authorize(&OP_PROJECT_ROLLBACK, Some(project_id))?;
        let git_ref = req
            .git_ref
            .as_deref()
            .map(|r| validate::git_ref("git_ref", r))
            .transpose()?;
        let (_, target, project) = self.rollback_target(project_id, req).await?;
        let (target_ref, seed) = match (&target, git_ref) {
            (Some(t), _) => {
                let tref = if project.is_git() {
                    t.commit_sha.clone()
                } else {
                    t.image_digest.clone()
                };
                let Some(tref) = tref else {
                    return Err(CoreError::conflict(
                        "the target Deployment recorded no commit or image digest to restore",
                    ));
                };
                let env = t
                    .env_snapshot_enc
                    .as_deref()
                    .map(|e| {
                        self.ctx
                            .secrets
                            .open("deployments", "env_snapshot_enc", &t.id, e)
                    })
                    .transpose()?;
                (
                    Some(tref),
                    Some(RollbackSeed {
                        from_id: t.id.clone(),
                        compose: t.compose_snapshot.clone(),
                        env,
                    }),
                )
            }
            (None, Some(r)) => {
                if !project.is_git() {
                    return Err(CoreError::validation(
                        "git_ref",
                        "only Git projects can roll back to a ref",
                    ));
                }
                (Some(r), None)
            }
            (None, None) => unreachable!("rollback_target guarantees a target"),
        };
        let from = seed.as_ref().map(|s| s.from_id.clone());
        let (row, reused) = self
            .enqueue(
                &AuditActor::from_actor(actor),
                NewDeployment {
                    project_id: project_id.to_owned(),
                    trigger: "rollback",
                    triggered_by: triggered_by(actor),
                    target_ref,
                    pending_env: None,
                    rollback: seed,
                    audit_action: "deployment.rollback",
                    meta: json!({ "rollback_from": from }),
                },
            )
            .await?;
        // A rollback with an explicit ref has no snapshot source: mark it via target_ref only.
        Ok(Enqueued {
            deployment: row.into(),
            reused,
        })
    }

    // ---- webhooks ----------------------------------------------------------------

    fn hook_actor(
        kind: &'static str,
        label: &str,
        channel: Channel,
        origin: &Origin,
    ) -> AuditActor {
        AuditActor {
            actor_type: kind,
            actor_id: None,
            label: label.to_owned(),
            channel,
            ip: origin.ip.clone(),
            request_id: origin.request_id.clone(),
        }
    }

    async fn denied(&self, who: &AuditActor, project_id: &str, reason: &str) {
        let ev = AuditEvent::new("deployment.trigger", "deployment")
            .project(project_id)
            .denied()
            .meta(json!({ "reason": reason }));
        // The Project may not exist (unknown id): then there is nothing to attach the row to.
        let known = match self.ctx.db.reader().acquire().await {
            Ok(mut c) => projects::get(&mut c, project_id)
                .await
                .ok()
                .flatten()
                .is_some(),
            Err(_) => false,
        };
        let ev = if known {
            ev
        } else {
            AuditEvent {
                project_id: None,
                ..ev
            }
        };
        if let Err(err) = audit::write_standalone(&self.ctx, who, &ev).await {
            tracing::warn!(error = %err, "could not record a denied webhook");
        }
    }

    /// `POST /deploy?id=`: bearer Deploy Token, body ignored (04 1.4).
    pub async fn deploy_webhook(
        &self,
        origin: &Origin,
        project_id: &str,
        bearer: &str,
    ) -> Result<Enqueued, CoreError> {
        let who = Self::hook_actor("deploy_token", "deploy-token", Channel::DeployToken, origin);
        let key = format!("{}|{project_id}", origin.ip.as_deref().unwrap_or("unknown"));
        let now = self.ctx.now();
        self.hooks
            .deploy
            .check(&key, now)
            .map_err(CoreError::rate_limited)?;
        self.hooks.deploy.record_failure(&key, now);
        let project = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            projects::get(&mut conn, project_id).await?
        };
        // Same work for unknown ids and wrong tokens: constant-time compare against a dummy.
        let stored = project
            .as_ref()
            .map_or("0".repeat(64), |p| p.deploy_token_hash.clone());
        let ok = ct_eq(&sha256_hex(bearer), &stored) && project.is_some() && !bearer.is_empty();
        let Some(project) = project.filter(|_| ok) else {
            self.denied(&who, project_id, "invalid deploy token").await;
            return Err(CoreError::unauthenticated("invalid deploy token"));
        };
        if !project.manual_deploy_enabled {
            self.denied(&who, project_id, "manual deploy disabled")
                .await;
            return Err(CoreError::forbidden(
                "manual deploys are disabled for this project",
            ));
        }
        let (row, reused) = self
            .enqueue(
                &who,
                NewDeployment {
                    project_id: project.id.clone(),
                    trigger: "deploy_token",
                    triggered_by: format!("deploy-token:{}", project.id),
                    target_ref: None,
                    pending_env: None,
                    rollback: None,
                    audit_action: "deployment.trigger",
                    meta: json!({}),
                },
            )
            .await?;
        Ok(Enqueued {
            deployment: row.into(),
            reused,
        })
    }

    /// `POST /webhooks/github?id=` (04 1.4). The caller has already capped the body.
    pub async fn github_webhook(
        &self,
        origin: &Origin,
        project_id: &str,
        d: &GithubDelivery<'_>,
    ) -> Result<GithubOutcome, CoreError> {
        let who = Self::hook_actor("github", "github", Channel::Webhook, origin);
        // Keyed by IP and Project (03 a.4): keyed by Project alone, anyone who knows the webhook
        // URL could burn the budget with unsigned requests and block GitHub's real deliveries.
        let key = format!("{}|{project_id}", origin.ip.as_deref().unwrap_or("unknown"));
        let now = self.ctx.now();
        self.hooks
            .github
            .check(&key, now)
            .map_err(CoreError::rate_limited)?;
        self.hooks.github.record_failure(&key, now);
        if d.body.len() > GITHUB_BODY_CAP {
            return Err(CoreError::validation("body", "payload too large"));
        }
        let project = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            projects::get(&mut conn, project_id).await?
        };
        let Some(project) = project else {
            return Err(CoreError::unauthenticated("invalid signature"));
        };
        let token = self.ctx.secrets.open(
            "projects",
            "deploy_token_enc",
            &project.id,
            &project.deploy_token_enc,
        )?;
        let supplied = d
            .signature
            .and_then(|s| s.strip_prefix("sha256="))
            .map(str::to_ascii_lowercase);
        let expected = hmac_hex(token.expose(), d.body);
        if !supplied.is_some_and(|s| ct_eq(&s, &expected)) {
            self.denied(&who, project_id, "invalid signature").await;
            return Err(CoreError::unauthenticated("invalid signature"));
        }
        match d.event {
            Some("ping") => return Ok(GithubOutcome::Pong),
            Some("push") => {}
            _ => return Ok(GithubOutcome::Ignored("not a push event")),
        }
        if let Some(id) = d.delivery_id {
            let mut seen = self.hooks.seen.lock().expect("seen");
            if seen.iter().any(|s| s == id) {
                return Ok(GithubOutcome::Duplicate);
            }
            if seen.len() == 100 {
                seen.pop_front();
            }
            seen.push_back(id.to_owned());
        }
        let payload: Value = if d.form_encoded {
            form_urlencoded::parse(d.body)
                .find(|(k, _)| k == "payload")
                .and_then(|(_, v)| serde_json::from_str(&v).ok())
        } else {
            serde_json::from_slice(d.body).ok()
        }
        .ok_or_else(|| CoreError::validation("body", "payload is not valid JSON"))?;
        if payload.get("deleted").and_then(Value::as_bool) == Some(true) {
            return Ok(GithubOutcome::Ignored("branch deleted"));
        }
        if !project.is_git() {
            return Ok(GithubOutcome::Ignored("image project"));
        }
        if !project.github_webhook_enabled {
            return Ok(GithubOutcome::Ignored("github webhook disabled"));
        }
        let want = format!(
            "refs/heads/{}",
            project.branch.as_deref().unwrap_or("master")
        );
        if payload.get("ref").and_then(Value::as_str) != Some(want.as_str()) {
            return Ok(GithubOutcome::Ignored("other branch"));
        }
        let after = payload.get("after").and_then(Value::as_str).unwrap_or("");
        let (row, reused) = self
            .enqueue(
                &who,
                NewDeployment {
                    project_id: project.id.clone(),
                    trigger: "github",
                    triggered_by: "github".into(),
                    target_ref: None,
                    pending_env: None,
                    rollback: None,
                    audit_action: "deployment.trigger",
                    meta: json!({
                        "delivery": d.delivery_id,
                        "ref": want,
                        "sha": after.chars().take(40).collect::<String>(),
                    }),
                },
            )
            .await?;
        Ok(GithubOutcome::Queued(Box::new(Enqueued {
            deployment: row.into(),
            reused,
        })))
    }
}

fn hmac_hex(key: &str, body: &[u8]) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key.as_bytes())
        .expect("hmac accepts any key length");
    mac.update(body);
    crate::crypto::hex(&mac.finalize().into_bytes())
}

/// Compute the `sha256=<hex>` header value GitHub would send (used by tests and tooling).
pub fn github_signature(secret: &str, body: &[u8]) -> String {
    format!("sha256={}", hmac_hex(secret, body))
}

/// Line diff via LCS; large inputs fall back to a whole-file replace marker.
fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a == b {
        return Vec::new();
    }
    if a.len() > 1500 || b.len() > 1500 {
        return vec![DiffLine {
            kind: '~',
            text: "files differ (too large to diff line by line)".into(),
        }];
    }
    let mut lcs = vec![vec![0u16; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push(DiffLine {
                kind: ' ',
                text: a[i].to_owned(),
            });
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(DiffLine {
                kind: '-',
                text: a[i].to_owned(),
            });
            i += 1;
        } else {
            out.push(DiffLine {
                kind: '+',
                text: b[j].to_owned(),
            });
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| DiffLine {
        kind: '-',
        text: (*l).to_owned(),
    }));
    out.extend(b[j..].iter().map(|l| DiffLine {
        kind: '+',
        text: (*l).to_owned(),
    }));
    // Keep two lines of context around changes.
    let keep: Vec<bool> = (0..out.len())
        .map(|k| {
            let lo = k.saturating_sub(2);
            let hi = (k + 2).min(out.len() - 1);
            out[lo..=hi].iter().any(|l| l.kind != ' ')
        })
        .collect();
    out.into_iter()
        .zip(keep)
        .filter_map(|(l, k)| k.then_some(l))
        .collect()
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn line_diff_marks_changes_with_context() {
        let d = line_diff("a\nb\nc\nd\ne\nf\ng\n", "a\nb\nc\nX\ne\nf\ng\n");
        let kinds: String = d.iter().map(|l| l.kind).collect();
        assert!(kinds.contains('-') && kinds.contains('+'));
        assert!(line_diff("same\n", "same\n").is_empty());
    }

    #[test]
    fn signature_has_the_github_shape() {
        let s = github_signature("secret", b"body");
        assert!(s.starts_with("sha256=") && s.len() == 7 + 64);
    }
}
