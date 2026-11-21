//! System (docs/rewrite/04 section 3): host usage, Docker info and disk usage, the Docker
//! prune with preview, and the dependency health report. Everything except `health` is
//! Owner-only (03 a.2: `system_*`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use super::containers::{ContainerProjects, map_docker_err};
use crate::actor::{Access, Actor, OpSpec, Scope};
use crate::applog::{AppLogLine, LogBuffer};
use crate::db::repo::db_err;
use crate::error::CoreError;
use crate::ports::{
    CancellationToken, CommandPort, CommandSpec, DiskUsage, DockerInfo, DockerPort, HostStatsPort,
    HostUsage, NullSink, PruneCandidate, PruneKind, PruneOutcome,
};

pub const OP_SYSTEM_USAGE: OpSpec = OpSpec::new("system_usage", Access::Owner, Scope::Read);
pub const OP_SYSTEM_CLEANUP_PREVIEW: OpSpec =
    OpSpec::new("system_cleanup_preview", Access::Owner, Scope::Read);
pub const OP_SYSTEM_CLEANUP: OpSpec = OpSpec::new("system_cleanup", Access::Owner, Scope::Admin);
pub const OP_HEALTH: OpSpec = OpSpec::new("health", Access::Authenticated, Scope::Read);
/// Recent Yanto application log (Owner only; the lines can describe other users' activity).
pub const OP_SYSTEM_LOGS: OpSpec = OpSpec::new("system_logs", Access::Owner, Scope::Read);
pub const OP_SYSTEM_LOGS_STREAM: OpSpec =
    OpSpec::new("system_logs_stream", Access::Owner, Scope::Read);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_SYSTEM_LOGS,
    &OP_SYSTEM_LOGS_STREAM,
    &OP_SYSTEM_USAGE,
    &OP_SYSTEM_CLEANUP_PREVIEW,
    &OP_SYSTEM_CLEANUP,
    &OP_HEALTH,
];

/// Each health probe has its own deadline (04 3.1).
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Free-space fraction under which the data dir disk reports `degraded`.
const LOW_DISK_FRACTION: f64 = 0.05;
/// Typed confirmation required when stopped containers are part of a prune.
pub const CONFIRM_CONTAINERS: &str = "prune containers";

#[derive(Debug, Clone, Serialize)]
pub struct DockerSection {
    pub info: Option<DockerInfo>,
    pub disk_usage: Option<DiskUsage>,
    /// Why the Docker panels are empty (Docker down); host stats still render.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageReport {
    pub host: HostUsage,
    pub docker: DockerSection,
}

#[derive(Debug, Clone, Serialize)]
pub struct PruneItem {
    pub id: String,
    pub name: String,
    pub size: Option<u64>,
    /// Owning Project of a stopped container, when it has one.
    pub project: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PruneGroup {
    pub kind: PruneKind,
    pub items: Vec<PruneItem>,
    pub total_size: u64,
    /// Whether the dashboard pre-selects this group (containers are off by default).
    pub default_selected: bool,
    /// Needs the typed confirmation before it can run.
    pub needs_confirm: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanupPreview {
    /// Volumes are never candidates (04 3.1), so there is no volumes group.
    pub groups: Vec<PruneGroup>,
}

#[derive(Debug, Clone, Default)]
pub struct CleanupSelection {
    pub build_cache: bool,
    pub images: bool,
    pub networks: bool,
    pub containers: bool,
    /// Must equal [`CONFIRM_CONTAINERS`] when `containers` is set.
    pub confirm: Option<String>,
}

impl CleanupSelection {
    /// The defaults of 04 3.1: build cache, dangling images and unused networks.
    pub fn defaults() -> Self {
        Self {
            build_cache: true,
            images: true,
            networks: true,
            containers: false,
            confirm: None,
        }
    }

    fn selected(&self, k: PruneKind) -> bool {
        match k {
            PruneKind::BuildCache => self.build_cache,
            PruneKind::Images => self.images,
            PruneKind::Networks => self.networks,
            PruneKind::Containers => self.containers,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanupStep {
    pub kind: PruneKind,
    pub ok: bool,
    pub deleted: u64,
    pub reclaimed_bytes: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanupResult {
    pub steps: Vec<CleanupStep>,
    pub reclaimed_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
    Degraded,
    Down,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthCheck {
    pub name: &'static str,
    pub status: HealthStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub status: HealthStatus,
    pub checks: Vec<HealthCheck>,
}

/// Extra health checks owned by other areas (Host Daemons: `frps`, `cloudflared`, Traefik; the
/// Hosted Registry). Register one with [`SystemService::register_health`] in `Services::new`;
/// its checks join the report and each runs under the same 3 s deadline.
#[async_trait::async_trait]
pub trait HealthContributor: Send + Sync + 'static {
    async fn checks(&self) -> Vec<HealthCheck>;
}

/// `docker compose version` (the Compose v2 plugin; 04 1.6 "Compose plugin missing").
pub struct ComposeHealth {
    command: Arc<dyn CommandPort>,
}

impl ComposeHealth {
    pub fn new(command: Arc<dyn CommandPort>) -> Self {
        Self { command }
    }
}

#[async_trait::async_trait]
impl HealthContributor for ComposeHealth {
    async fn checks(&self) -> Vec<HealthCheck> {
        let spec = CommandSpec::new("docker", &["compose", "version", "--short"])
            .capture()
            .timeout(PROBE_TIMEOUT);
        let out = self
            .command
            .run(spec, Arc::new(NullSink), &CancellationToken::new())
            .await;
        let c = match out {
            Ok(o) if o.success() && !o.stdout.trim().is_empty() => check(
                "compose",
                HealthStatus::Ok,
                format!("docker compose {}", o.stdout.trim()),
            ),
            Ok(o) if o.timed_out => check("compose", HealthStatus::Degraded, "timed out"),
            // Deployments cannot run without the plugin, but the dashboard still works.
            _ => check("compose", HealthStatus::Degraded, "docker compose: missing"),
        };
        vec![c]
    }
}

type Contributors = Arc<std::sync::Mutex<Vec<Arc<dyn HealthContributor>>>>;

#[derive(Clone)]
pub struct SystemService {
    ctx: Ctx,
    docker: Arc<dyn DockerPort>,
    host: Arc<dyn HostStatsPort>,
    projects: Arc<dyn ContainerProjects>,
    data_dir: PathBuf,
    logs: Arc<LogBuffer>,
    contributors: Contributors,
}

impl std::fmt::Debug for SystemService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemService").finish_non_exhaustive()
    }
}

impl SystemService {
    pub(super) fn new(
        ctx: Ctx,
        docker: Arc<dyn DockerPort>,
        host: Arc<dyn HostStatsPort>,
        projects: Arc<dyn ContainerProjects>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            ctx,
            docker,
            host,
            projects,
            data_dir,
            logs: LogBuffer::global(),
            contributors: Contributors::default(),
        }
    }

    /// Read another buffer than the process-wide one (tests).
    pub fn with_log_buffer(mut self, logs: Arc<LogBuffer>) -> Self {
        self.logs = logs;
        self
    }

    /// Add checks to `health`. Shared by every clone of the service.
    pub fn register_health(&self, c: Arc<dyn HealthContributor>) {
        self.contributors.lock().expect("contributors").push(c);
    }

    /// The newest `tail` lines of the application log (default and cap 500), oldest first.
    pub async fn logs(
        &self,
        actor: &Actor,
        tail: Option<usize>,
    ) -> Result<Vec<AppLogLine>, CoreError> {
        actor.authorize(&OP_SYSTEM_LOGS, None)?;
        Ok(self.logs.tail(
            tail.unwrap_or(crate::applog::CAPACITY)
                .min(crate::applog::CAPACITY),
        ))
    }

    /// The last `backlog` lines and a receiver for everything logged afterwards.
    pub async fn logs_stream(
        &self,
        actor: &Actor,
        backlog: Option<usize>,
    ) -> Result<
        (
            Vec<AppLogLine>,
            tokio::sync::broadcast::Receiver<AppLogLine>,
        ),
        CoreError,
    > {
        actor.authorize(&OP_SYSTEM_LOGS_STREAM, None)?;
        Ok(self.logs.follow(
            backlog
                .unwrap_or(crate::applog::CAPACITY)
                .min(crate::applog::CAPACITY),
        ))
    }

    pub async fn usage(&self, actor: &Actor) -> Result<UsageReport, CoreError> {
        actor.authorize(&OP_SYSTEM_USAGE, None)?;
        let (info, df) = tokio::join!(
            tokio::time::timeout(PROBE_TIMEOUT, self.docker.info()),
            tokio::time::timeout(PROBE_TIMEOUT, self.docker.disk_usage()),
        );
        let info =
            info.unwrap_or_else(|_| Err(crate::ports::DockerError::Unavailable("timeout".into())));
        let df = df.ok().and_then(Result::ok);
        let mut paths = vec![
            PathBuf::from("/"),
            self.data_dir.clone(),
            self.data_dir.join("projects"),
        ];
        let docker = match info {
            Ok(i) => {
                paths.push(PathBuf::from(&i.root_dir));
                DockerSection {
                    info: Some(i),
                    disk_usage: df,
                    error: None,
                }
            }
            Err(e) => DockerSection {
                info: None,
                disk_usage: None,
                error: Some(e.to_string()),
            },
        };
        let host = self.host.usage(&paths).await;
        Ok(UsageReport { host, docker })
    }

    /// Read-only candidates per prune kind (never volumes).
    pub async fn cleanup_preview(&self, actor: &Actor) -> Result<CleanupPreview, CoreError> {
        actor.authorize(&OP_SYSTEM_CLEANUP_PREVIEW, None)?;
        let raw = self.docker.prune_preview().await.map_err(map_docker_err)?;
        let mut groups = Vec::new();
        for kind in PruneKind::ORDER {
            let cands: &Vec<PruneCandidate> = match kind {
                PruneKind::BuildCache => &raw.build_cache,
                PruneKind::Images => &raw.images,
                PruneKind::Networks => &raw.networks,
                PruneKind::Containers => &raw.containers,
            };
            let mut items = Vec::with_capacity(cands.len());
            for c in cands {
                let project = if kind == PruneKind::Containers {
                    self.project_of_labels(c).await?
                } else {
                    None
                };
                items.push(PruneItem {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    size: c.size,
                    project,
                });
            }
            groups.push(PruneGroup {
                kind,
                total_size: items.iter().filter_map(|i| i.size).sum(),
                items,
                default_selected: kind != PruneKind::Containers,
                needs_confirm: kind == PruneKind::Containers,
            });
        }
        Ok(CleanupPreview { groups })
    }

    async fn project_of_labels(&self, c: &PruneCandidate) -> Result<Option<String>, CoreError> {
        // The mapping hook takes a container; a candidate carries just its labels.
        let info = crate::ports::ContainerInfo {
            id: c.id.clone(),
            name: c.name.clone(),
            image: String::new(),
            state: "exited".into(),
            status: String::new(),
            created: 0,
            ports: vec![],
            labels: c.labels.clone(),
        };
        self.projects.project_for(&info).await
    }

    /// Run the selected prune steps in order; a failing step is reported and later steps still run.
    pub async fn cleanup(
        &self,
        actor: &Actor,
        sel: &CleanupSelection,
    ) -> Result<CleanupResult, CoreError> {
        actor.authorize(&OP_SYSTEM_CLEANUP, None)?;
        if !PruneKind::ORDER.iter().any(|k| sel.selected(*k)) {
            return Err(CoreError::validation(
                "selection",
                "select at least one item to prune",
            ));
        }
        if sel.containers && sel.confirm.as_deref().map(str::trim) != Some(CONFIRM_CONTAINERS) {
            return Err(CoreError::validation(
                "confirm",
                format!("type \"{CONFIRM_CONTAINERS}\" to remove stopped containers"),
            ));
        }
        let mut steps = Vec::new();
        for kind in PruneKind::ORDER.into_iter().filter(|k| sel.selected(*k)) {
            let step = match self.docker.prune(kind).await {
                Ok(PruneOutcome {
                    deleted,
                    reclaimed_bytes,
                }) => CleanupStep {
                    kind,
                    ok: true,
                    deleted,
                    reclaimed_bytes,
                    error: None,
                },
                Err(e) => CleanupStep {
                    kind,
                    ok: false,
                    deleted: 0,
                    reclaimed_bytes: 0,
                    error: Some(e.to_string()),
                },
            };
            steps.push(step);
        }
        let reclaimed_bytes = steps.iter().map(|s| s.reclaimed_bytes).sum();
        let mut tx = self.ctx.db.begin_write().await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("system.cleanup", "system").meta(json!({
                "selected": steps.iter().map(|s| s.kind.as_str()).collect::<Vec<_>>(),
                "failed": steps.iter().filter(|s| !s.ok).map(|s| s.kind.as_str()).collect::<Vec<_>>(),
                "deleted": steps.iter().map(|s| s.deleted).sum::<u64>(),
                "reclaimed_bytes": reclaimed_bytes,
            })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(CleanupResult {
            steps,
            reclaimed_bytes,
        })
    }

    /// Independent probes (database, Docker, data-dir disk), each with a 3 s deadline.
    pub async fn health(&self, actor: &Actor) -> Result<HealthReport, CoreError> {
        actor.authorize(&OP_HEALTH, None)?;
        let (db, docker, disk) =
            tokio::join!(self.probe_db(), self.probe_docker(), self.probe_disk());
        let mut checks = vec![db, docker, disk];
        let extra: Vec<Arc<dyn HealthContributor>> =
            self.contributors.lock().expect("contributors").clone();
        let results = futures_join_all(extra.into_iter().map(|c| async move {
            tokio::time::timeout(PROBE_TIMEOUT, c.checks())
                .await
                .unwrap_or_else(|_| vec![check("extra", HealthStatus::Degraded, "timed out")])
        }))
        .await;
        checks.extend(results.into_iter().flatten());
        let status = if checks
            .iter()
            .any(|c| c.name == "database" && c.status == HealthStatus::Down)
        {
            HealthStatus::Down
        } else if checks.iter().any(|c| c.status != HealthStatus::Ok) {
            HealthStatus::Degraded
        } else {
            HealthStatus::Ok
        };
        Ok(HealthReport { status, checks })
    }

    async fn probe_db(&self) -> HealthCheck {
        match tokio::time::timeout(PROBE_TIMEOUT, self.ctx.db.ping()).await {
            Ok(Ok(())) => check("database", HealthStatus::Ok, "SELECT 1 ok"),
            Ok(Err(_)) => check("database", HealthStatus::Down, "query failed"),
            Err(_) => check("database", HealthStatus::Down, "timed out"),
        }
    }

    async fn probe_docker(&self) -> HealthCheck {
        let probe = async {
            self.docker.ping().await?;
            self.docker.info().await
        };
        match tokio::time::timeout(PROBE_TIMEOUT, probe).await {
            Ok(Ok(i)) => check(
                "docker",
                HealthStatus::Ok,
                format!("engine {}", i.server_version),
            ),
            Ok(Err(e)) => check("docker", HealthStatus::Degraded, e.to_string()),
            Err(_) => check("docker", HealthStatus::Degraded, "timed out"),
        }
    }

    async fn probe_disk(&self) -> HealthCheck {
        let usage = tokio::time::timeout(
            PROBE_TIMEOUT,
            self.host.usage(std::slice::from_ref(&self.data_dir)),
        )
        .await;
        let Ok(usage) = usage else {
            return check("disk", HealthStatus::Degraded, "timed out");
        };
        let key = self.data_dir.to_string_lossy();
        let disk = usage
            .disks
            .iter()
            .find(|d| d.paths.iter().any(|p| p.as_str() == key))
            .or_else(|| usage.disks.first());
        match disk {
            Some(d) if d.total_bytes > 0 => {
                let free = d.available_bytes as f64 / d.total_bytes as f64;
                let detail = format!(
                    "{} free on {}",
                    super::fmt_bytes(d.available_bytes),
                    d.mount
                );
                let status = if free < LOW_DISK_FRACTION {
                    HealthStatus::Degraded
                } else {
                    HealthStatus::Ok
                };
                check("disk", status, detail)
            }
            _ => check("disk", HealthStatus::Degraded, "disk not found"),
        }
    }
}

fn check(name: &'static str, status: HealthStatus, detail: impl Into<String>) -> HealthCheck {
    HealthCheck {
        name,
        status,
        detail: detail.into(),
    }
}

/// Run the futures concurrently and collect their outputs in order.
async fn futures_join_all<F, T>(it: impl IntoIterator<Item = F>) -> Vec<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handles: Vec<_> = it.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(v) = h.await {
            out.push(v);
        }
    }
    out
}
