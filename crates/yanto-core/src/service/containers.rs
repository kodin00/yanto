//! Containers (docs/rewrite/04 section 2): list, inspect, start/stop/restart, logs (snapshot
//! and live follow) and the dashboard-only exec terminal.
//!
//! ## Authorization
//!
//! The container operations are declared `Access::Authenticated` in their [`OpSpec`]s (so the
//! API Token scope and `session_only` rules still come from `authorize`) and the per-container
//! rule is applied by [`ContainerService::authorize_container`]:
//!
//! * the container maps to a Project through [`ContainerProjects`] (Compose label
//!   `com.docker.compose.project == projects.slug`); the actor then needs the Project grant
//!   (`view` to read, `runtime` to act or open a terminal), exactly like a Project operation;
//! * a container with no Project is **Owner only**; a Member gets 404 (existence hidden).
//!
//! M2 wires the real mapping by passing its own [`ContainerProjects`] implementation in
//! `Services::new` (replacing [`NoProjects`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::actor::{Access, Actor, Need, OpSpec, Permission, Scope};
use crate::db::repo::db_err;
use crate::error::CoreError;
use crate::ports::{
    BoxStream, ContainerDetail, ContainerInfo, ContainerStats, DockerError, DockerPort, ExecIo,
    LogLine, MANAGED_LABEL,
};

pub const OP_CONTAINERS_LIST: OpSpec =
    OpSpec::new("containers_list", Access::Authenticated, Scope::Read);
pub const OP_CONTAINER_GET: OpSpec =
    OpSpec::new("container_get", Access::Authenticated, Scope::Read);
pub const OP_CONTAINER_LOGS: OpSpec =
    OpSpec::new("container_logs", Access::Authenticated, Scope::Read);
pub const OP_CONTAINER_LOGS_STREAM: OpSpec =
    OpSpec::new("container_logs_stream", Access::Authenticated, Scope::Read);
pub const OP_CONTAINER_START: OpSpec =
    OpSpec::new("container_start", Access::Authenticated, Scope::Write);
pub const OP_CONTAINER_STOP: OpSpec =
    OpSpec::new("container_stop", Access::Authenticated, Scope::Admin);
pub const OP_CONTAINER_RESTART: OpSpec =
    OpSpec::new("container_restart", Access::Authenticated, Scope::Admin);
/// Dashboard terminal only (D24): never reachable with an API Token.
pub const OP_CONTAINER_EXEC_SESSION: OpSpec = OpSpec::new(
    "container_exec_session",
    Access::Authenticated,
    Scope::Write,
)
.session_only();

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_CONTAINERS_LIST,
    &OP_CONTAINER_GET,
    &OP_CONTAINER_LOGS,
    &OP_CONTAINER_LOGS_STREAM,
    &OP_CONTAINER_START,
    &OP_CONTAINER_STOP,
    &OP_CONTAINER_RESTART,
    &OP_CONTAINER_EXEC_SESSION,
];

const CACHE_TTL_MS: i64 = 5_000;
const STATS_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_TIMEOUT_SECS: u32 = 10;
pub const DEFAULT_TAIL: u32 = 500;
const MAX_TAIL: u32 = 5_000;
/// Concurrent terminals per account and in total (04 2.2 step 6, ASSUMPTION values).
pub const EXEC_PER_ACCOUNT: usize = 3;
pub const EXEC_TOTAL: usize = 20;
/// How long after hangup a still-running exec is reported (OQ-25).
pub const EXEC_LINGER_CHECK: Duration = Duration::from_secs(10);

/// Maps a container to the Project that owns it. M2 implements this with the
/// `projects.slug` lookup; until Projects exist [`NoProjects`] returns `None` for everything,
/// which makes every container Owner-only.
#[async_trait::async_trait]
pub trait ContainerProjects: Send + Sync + 'static {
    /// The Project id owning `container`, if any.
    async fn project_for(&self, container: &ContainerInfo) -> Result<Option<String>, CoreError>;
}

/// The M3 default: no Projects, so no container belongs to one.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoProjects;

#[async_trait::async_trait]
impl ContainerProjects for NoProjects {
    async fn project_for(&self, _: &ContainerInfo) -> Result<Option<String>, CoreError> {
        Ok(None)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerDto {
    pub id: String,
    pub short_id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub created: i64,
    pub ports: Vec<String>,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    /// Owning Yanto Project (id), when the mapping hook finds one.
    pub project: Option<String>,
    pub cpu_percent: Option<f64>,
    pub mem_usage: Option<u64>,
    pub mem_limit: Option<u64>,
    /// Value of the `yanto.managed` label (the Hosted Registry is `registry`).
    pub managed: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerList {
    pub containers: Vec<ContainerDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerDetailDto {
    #[serde(flatten)]
    pub summary: ContainerDto,
    pub detail: ContainerDetail,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
}

impl ContainerAction {
    fn audit_action(self) -> &'static str {
        match self {
            ContainerAction::Start => "container.start",
            ContainerAction::Stop => "container.stop",
            ContainerAction::Restart => "container.restart",
        }
    }
}

struct CacheEntry {
    rev: u64,
    at_ms: i64,
    rows: Arc<Vec<ContainerDto>>,
}

#[derive(Default)]
struct Shared {
    /// Bumped by every mutation; a cached list from an older revision is discarded.
    rev: AtomicU64,
    /// The mutex doubles as the single in-flight fetch: concurrent readers queue behind it
    /// and then hit the fresh cache.
    cache: tokio::sync::Mutex<Option<CacheEntry>>,
    sessions: std::sync::Mutex<HashMap<String, usize>>,
}

#[derive(Clone)]
pub struct ContainerService {
    ctx: Ctx,
    docker: Arc<dyn DockerPort>,
    projects: Arc<dyn ContainerProjects>,
    shared: Arc<Shared>,
    linger_check: Duration,
}

impl std::fmt::Debug for ContainerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerService").finish_non_exhaustive()
    }
}

fn docker_err(e: DockerError) -> CoreError {
    match e {
        DockerError::NotFound => CoreError::not_found("container not found"),
        DockerError::Conflict(m) => CoreError::conflict(m),
        DockerError::Unavailable(_) => CoreError::unavailable("Docker unavailable", e),
        DockerError::Other(_) => CoreError::internal("docker request failed", e),
    }
}

pub(super) fn map_docker_err(e: DockerError) -> CoreError {
    docker_err(e)
}

/// Container ids and names reach the Engine URL path: allow only what Docker itself allows.
fn validate_ref(id: &str) -> Result<(), CoreError> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if ok {
        Ok(())
    } else {
        Err(CoreError::not_found("container not found"))
    }
}

fn format_port(p: &crate::ports::PortInfo) -> String {
    match p.public_port {
        Some(public) => format!(
            "{}:{}->{}/{}",
            p.ip.as_deref().unwrap_or("0.0.0.0"),
            public,
            p.private_port,
            p.proto
        ),
        None => format!("{}/{}", p.private_port, p.proto),
    }
}

fn dto(c: &ContainerInfo, project: Option<String>, stats: Option<ContainerStats>) -> ContainerDto {
    ContainerDto {
        id: c.id.clone(),
        short_id: c.id.chars().take(12).collect(),
        name: c.name.clone(),
        image: c.image.clone(),
        state: c.state.clone(),
        status: c.status.clone(),
        created: c.created,
        ports: c.ports.iter().map(format_port).collect(),
        compose_project: c.compose_project().map(str::to_owned),
        compose_service: c.compose_service().map(str::to_owned),
        project,
        cpu_percent: stats.map(|s| s.cpu_percent),
        mem_usage: stats.map(|s| s.mem_usage),
        mem_limit: stats.map(|s| s.mem_limit),
        managed: c.labels.get(MANAGED_LABEL).cloned(),
    }
}

impl ContainerService {
    pub(super) fn new(
        ctx: Ctx,
        docker: Arc<dyn DockerPort>,
        projects: Arc<dyn ContainerProjects>,
    ) -> Self {
        Self {
            ctx,
            docker,
            projects,
            shared: Arc::default(),
            linger_check: EXEC_LINGER_CHECK,
        }
    }

    /// Use a different container-to-Project mapping (M2 wires its `projects.slug` lookup here
    /// or in `Services::new`; tests use it to model Projects).
    pub fn with_projects(mut self, projects: Arc<dyn ContainerProjects>) -> Self {
        self.projects = projects;
        self
    }

    /// Override the post-hangup lingering check delay (tests).
    pub fn with_linger_check(mut self, d: Duration) -> Self {
        self.linger_check = d;
        self
    }

    fn invalidate(&self) {
        self.shared.rev.fetch_add(1, Ordering::SeqCst);
    }

    /// Drop the cached container list. Deployments (M2) call this after every run.
    pub fn invalidate_cache(&self) {
        self.invalidate();
    }

    /// Apply the container rule (see module docs). `need` is `View` for reads, `runtime` for actions.
    async fn authorize_container(
        &self,
        actor: &Actor,
        op: &OpSpec,
        info: &ContainerInfo,
        need: Need,
    ) -> Result<Option<String>, CoreError> {
        actor.authorize(op, None)?;
        let project = self.projects.project_for(info).await?;
        match &project {
            Some(p) => {
                let spec = OpSpec {
                    access: Access::Project(need),
                    ..*op
                };
                actor.authorize(&spec, Some(p))?;
            }
            None if actor.is_owner() => {}
            // Containers outside any Project are Owner-only; hide them from Members.
            None => return Err(CoreError::not_found("container not found")),
        }
        Ok(project)
    }

    async fn resolve(&self, id: &str) -> Result<ContainerInfo, CoreError> {
        validate_ref(id)?;
        let detail = self
            .docker
            .inspect_container(id)
            .await
            .map_err(docker_err)?;
        Ok(detail.info)
    }

    /// All containers visible to `actor`, newest first, with CPU and memory for running ones.
    pub async fn list(&self, actor: &Actor) -> Result<ContainerList, CoreError> {
        actor.authorize(&OP_CONTAINERS_LIST, None)?;
        let rows = self.cached_rows().await?;
        let visible = actor.visible_projects();
        let containers = rows
            .iter()
            .filter(|r| match (&visible, &r.project) {
                (None, _) => true,
                (Some(v), Some(p)) => v.contains(&p.as_str()),
                (Some(_), None) => false,
            })
            .cloned()
            .collect();
        Ok(ContainerList { containers })
    }

    async fn cached_rows(&self) -> Result<Arc<Vec<ContainerDto>>, CoreError> {
        let mut cache = self.shared.cache.lock().await;
        let rev = self.shared.rev.load(Ordering::SeqCst);
        let now = self.ctx.now();
        if let Some(e) = cache.as_ref()
            && e.rev == rev
            && now - e.at_ms < CACHE_TTL_MS
        {
            return Ok(e.rows.clone());
        }
        let list = self.docker.list_containers().await.map_err(docker_err)?;
        let stats = futures_util::future::join_all(list.iter().map(|c| async move {
            if !c.is_running() {
                return None;
            }
            // A slow `stats` must never block the list (04 2.4).
            match tokio::time::timeout(STATS_TIMEOUT, self.docker.container_stats(&c.id)).await {
                Ok(Ok(s)) => s,
                _ => None,
            }
        }))
        .await;
        let mut rows = Vec::with_capacity(list.len());
        for (c, s) in list.iter().zip(stats) {
            let project = self.projects.project_for(c).await?;
            rows.push(dto(c, project, s));
        }
        let rows = Arc::new(rows);
        *cache = Some(CacheEntry {
            rev,
            at_ms: now,
            rows: rows.clone(),
        });
        Ok(rows)
    }

    pub async fn get(&self, actor: &Actor, id: &str) -> Result<ContainerDetailDto, CoreError> {
        actor.authorize(&OP_CONTAINER_GET, None)?;
        validate_ref(id)?;
        let detail = self
            .docker
            .inspect_container(id)
            .await
            .map_err(docker_err)?;
        let project = self
            .authorize_container(actor, &OP_CONTAINER_GET, &detail.info, Need::View)
            .await?;
        Ok(ContainerDetailDto {
            summary: dto(&detail.info, project, None),
            detail,
        })
    }

    pub async fn start(&self, actor: &Actor, id: &str) -> Result<(), CoreError> {
        self.act(actor, id, ContainerAction::Start, &OP_CONTAINER_START)
            .await
    }

    pub async fn stop(&self, actor: &Actor, id: &str) -> Result<(), CoreError> {
        self.act(actor, id, ContainerAction::Stop, &OP_CONTAINER_STOP)
            .await
    }

    pub async fn restart(&self, actor: &Actor, id: &str) -> Result<(), CoreError> {
        self.act(actor, id, ContainerAction::Restart, &OP_CONTAINER_RESTART)
            .await
    }

    async fn act(
        &self,
        actor: &Actor,
        id: &str,
        action: ContainerAction,
        op: &OpSpec,
    ) -> Result<(), CoreError> {
        actor.authorize(op, None)?;
        let info = self.resolve(id).await?;
        let project = self
            .authorize_container(actor, op, &info, Need::Permission(Permission::Runtime))
            .await?;
        if info.labels.get(MANAGED_LABEL).map(String::as_str) == Some("registry") {
            return Err(CoreError::conflict(
                "the Hosted Registry container is controlled from the Registry page",
            ));
        }
        let res = match action {
            ContainerAction::Start => self.docker.start_container(&info.id).await,
            ContainerAction::Stop => {
                self.docker
                    .stop_container(&info.id, STOP_TIMEOUT_SECS)
                    .await
            }
            ContainerAction::Restart => {
                self.docker
                    .restart_container(&info.id, STOP_TIMEOUT_SECS)
                    .await
            }
        };
        self.invalidate();
        res.map_err(docker_err)?;
        let mut tx = self.ctx.db.begin_write().await?;
        let mut ev = AuditEvent::new(action.audit_action(), "container")
            .entity(info.id.clone())
            .meta(json!({ "name": info.name }));
        if let Some(p) = project {
            ev = ev.project(p);
        }
        audit::write(&self.ctx, &mut tx, &AuditActor::from_actor(actor), &ev).await?;
        tx.commit().await.map_err(db_err)
    }

    /// Last `tail` log lines (default 500).
    pub async fn logs(
        &self,
        actor: &Actor,
        id: &str,
        tail: Option<u32>,
    ) -> Result<Vec<LogLine>, CoreError> {
        actor.authorize(&OP_CONTAINER_LOGS, None)?;
        let info = self.resolve(id).await?;
        self.authorize_container(actor, &OP_CONTAINER_LOGS, &info, Need::View)
            .await?;
        let tail = tail.unwrap_or(DEFAULT_TAIL).min(MAX_TAIL);
        self.docker.logs(&info.id, tail).await.map_err(docker_err)
    }

    /// Live log follow from `since` (unix seconds; `None` = now).
    pub async fn logs_stream(
        &self,
        actor: &Actor,
        id: &str,
        since: Option<i64>,
    ) -> Result<BoxStream<Result<LogLine, CoreError>>, CoreError> {
        actor.authorize(&OP_CONTAINER_LOGS_STREAM, None)?;
        let info = self.resolve(id).await?;
        self.authorize_container(actor, &OP_CONTAINER_LOGS_STREAM, &info, Need::View)
            .await?;
        let since = since.unwrap_or_else(|| self.ctx.now() / 1000);
        let inner = self.docker.logs_follow(&info.id, since);
        Ok(Box::pin(futures_util::StreamExt::map(inner, |r| {
            r.map_err(docker_err)
        })))
    }

    /// Open an interactive shell in a running container. Returns the attached TTY and a ticket the
    /// adapter must `finish` when the session ends. Audited at start and end.
    pub async fn exec_open(
        &self,
        actor: &Actor,
        id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(ExecIo, ExecTicket), CoreError> {
        actor.authorize(&OP_CONTAINER_EXEC_SESSION, None)?;
        let info = self.resolve(id).await?;
        let project = self
            .authorize_container(
                actor,
                &OP_CONTAINER_EXEC_SESSION,
                &info,
                Need::Permission(Permission::Runtime),
            )
            .await?;
        if !info.is_running() {
            return Err(CoreError::conflict("container is not running"));
        }
        let key = actor.user_id.clone().unwrap_or_else(|| actor.label.clone());
        let guard = self.acquire_session(&key)?;
        let io = self
            .docker
            .exec_start(&info.id, cols.clamp(1, 1000), rows.clamp(1, 1000))
            .await
            .map_err(docker_err)?;
        let who = AuditActor::from_actor(actor);
        let mut ev = AuditEvent::new("container.exec_start", "container")
            .entity(info.id.clone())
            .meta(json!({ "name": info.name, "exec_id": io.exec_id }));
        if let Some(p) = &project {
            ev = ev.project(p.clone());
        }
        let mut tx = self.ctx.db.begin_write().await?;
        audit::write(&self.ctx, &mut tx, &who, &ev).await?;
        tx.commit().await.map_err(db_err)?;
        let ticket = ExecTicket {
            svc: self.clone(),
            exec_id: io.exec_id.clone(),
            container: info,
            project,
            who,
            started_ms: self.ctx.now(),
            _guard: guard,
        };
        Ok((io, ticket))
    }

    fn acquire_session(&self, key: &str) -> Result<SessionGuard, CoreError> {
        let mut map = self.shared.sessions.lock().expect("sessions lock");
        let total: usize = map.values().sum();
        let mine = map.get(key).copied().unwrap_or(0);
        if mine >= EXEC_PER_ACCOUNT || total >= EXEC_TOTAL {
            let mut e = CoreError::rate_limited(5);
            e.message = "too many open terminal sessions".into();
            return Err(e);
        }
        *map.entry(key.to_owned()).or_default() += 1;
        Ok(SessionGuard {
            shared: self.shared.clone(),
            key: key.to_owned(),
        })
    }

    /// Open terminal sessions (all accounts).
    pub fn open_exec_sessions(&self) -> usize {
        self.shared
            .sessions
            .lock()
            .expect("sessions lock")
            .values()
            .sum()
    }
}

struct SessionGuard {
    shared: Arc<Shared>,
    key: String,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let mut map = self.shared.sessions.lock().expect("sessions lock");
        if let Some(n) = map.get_mut(&self.key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                map.remove(&self.key);
            }
        }
    }
}

/// Handle for one open terminal; keeps its concurrency slot until dropped.
pub struct ExecTicket {
    svc: ContainerService,
    exec_id: String,
    container: ContainerInfo,
    project: Option<String>,
    who: AuditActor,
    started_ms: i64,
    _guard: SessionGuard,
}

impl ExecTicket {
    pub fn container_name(&self) -> &str {
        &self.container.name
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), CoreError> {
        self.svc
            .docker
            .exec_resize(&self.exec_id, cols.clamp(1, 1000), rows.clamp(1, 1000))
            .await
            .map_err(docker_err)
    }

    /// End the session: hang the shell up (closing `input`), audit the end with duration, byte
    /// counts and exit code, and schedule the lingering-process check (OQ-25).
    pub async fn finish(
        self,
        mut input: std::pin::Pin<Box<dyn tokio::io::AsyncWrite + Send>>,
        bytes_in: u64,
        bytes_out: u64,
        reason: &str,
    ) {
        let _ = input.shutdown().await;
        drop(input);
        let state = self.svc.docker.exec_inspect(&self.exec_id).await.ok();
        let duration_ms = self.svc.ctx.now() - self.started_ms;
        let mut meta = json!({
            "name": self.container.name,
            "exec_id": self.exec_id,
            "duration_ms": duration_ms,
            "bytes_in": bytes_in,
            "bytes_out": bytes_out,
            "reason": reason,
        });
        if let Some(code) = state.and_then(|s| s.exit_code) {
            meta["exit_code"] = json!(code);
        }
        let mut ev = AuditEvent::new("container.exec_end", "container")
            .entity(self.container.id.clone())
            .meta(meta);
        if let Some(p) = &self.project {
            ev = ev.project(p.clone());
        }
        if let Err(err) = audit::write_standalone(&self.svc.ctx, &self.who, &ev).await {
            tracing::error!(error = %err, "failed to audit exec session end");
        }
        let svc = self.svc.clone();
        let exec_id = self.exec_id.clone();
        let container = self.container.clone();
        let who = self.who.clone();
        let delay = self.svc.linger_check;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Ok(s) = svc.docker.exec_inspect(&exec_id).await
                && s.running
            {
                let mut ev = AuditEvent::new("container.exec_lingering", "container")
                    .entity(container.id.clone())
                    .meta(json!({ "name": container.name, "exec_id": exec_id }));
                ev.outcome = Outcome::Error;
                let _ = audit::write_standalone(&svc.ctx, &who, &ev).await;
            }
        });
    }
}
