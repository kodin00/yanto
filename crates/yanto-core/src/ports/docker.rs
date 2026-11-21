//! Docker Engine port (docs/rewrite/01 7.1, 04 sections 2 and 3). The bollard
//! implementation lives in `yanto-host`; tests use `testing::docker::FakeDocker`.
//!
//! The port speaks in plain domain types so services never see bollard models.

use std::collections::BTreeMap;
use std::pin::Pin;

use futures_core::Stream;
use serde::Serialize;
use tokio::io::AsyncWrite;

pub type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

/// Failure talking to the Docker Engine.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DockerError {
    #[error("no such container or exec")]
    NotFound,
    /// The Engine refused the request in the current state (for example exec on a stopped container).
    #[error("{0}")]
    Conflict(String),
    /// Socket unreachable or the daemon did not answer in time.
    #[error("docker unavailable: {0}")]
    Unavailable(String),
    #[error("docker error: {0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortInfo {
    pub ip: Option<String>,
    pub private_port: u16,
    pub public_port: Option<u16>,
    pub proto: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainerInfo {
    /// Full 64-hex id.
    pub id: String,
    pub name: String,
    pub image: String,
    /// `running`, `exited`, `paused`, ...
    pub state: String,
    pub status: String,
    /// Unix seconds.
    pub created: i64,
    pub ports: Vec<PortInfo>,
    pub labels: BTreeMap<String, String>,
}

impl ContainerInfo {
    pub fn is_running(&self) -> bool {
        self.state == "running"
    }

    pub fn compose_project(&self) -> Option<&str> {
        self.labels
            .get(COMPOSE_PROJECT_LABEL)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    pub fn compose_service(&self) -> Option<&str> {
        self.labels
            .get("com.docker.compose.service")
            .map(String::as_str)
    }
}

/// Compose sets this label to the Compose project name (`projects.slug` for Yanto Projects).
pub const COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";
/// Label on Yanto-managed helper containers (the Hosted Registry carries `registry`).
pub const MANAGED_LABEL: &str = "yanto.managed";

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ContainerStats {
    pub cpu_percent: f64,
    pub mem_usage: u64,
    pub mem_limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MountInfo {
    pub source: String,
    pub destination: String,
    pub read_write: bool,
}

/// `docker inspect`, without environment variables (they may hold secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainerDetail {
    pub info: ContainerInfo,
    pub command: String,
    pub restart_count: i64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub health: Option<String>,
    pub mounts: Vec<MountInfo>,
    pub networks: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogLine {
    pub stream: LogStream,
    /// RFC 3339 timestamp prefix added by the Engine (`timestamps=true`).
    pub timestamp: Option<String>,
    pub text: String,
}

/// An attached TTY exec: bytes in, bytes out. Dropping `input` hangs the shell up.
pub struct ExecIo {
    pub exec_id: String,
    pub output: BoxStream<Result<Vec<u8>, DockerError>>,
    pub input: Pin<Box<dyn AsyncWrite + Send>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecState {
    pub running: bool,
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DockerInfo {
    pub server_version: String,
    pub api_version: String,
    pub os: String,
    pub kernel: String,
    pub storage_driver: String,
    pub root_dir: String,
    pub containers: i64,
    pub containers_running: i64,
    pub images: i64,
    pub ncpu: i64,
    pub mem_total: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct UsageBucket {
    pub total_count: i64,
    pub active_count: i64,
    pub size: i64,
    pub reclaimable: i64,
}

/// `docker system df`. A bucket is `None` when the daemon did not report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct DiskUsage {
    pub images: Option<UsageBucket>,
    pub containers: Option<UsageBucket>,
    pub volumes: Option<UsageBucket>,
    pub build_cache: Option<UsageBucket>,
}

/// Which category a prune step removes. Volumes are deliberately not a kind (04 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PruneKind {
    BuildCache,
    Images,
    Networks,
    Containers,
}

impl PruneKind {
    /// Execution order (04 3.1): build prune, image prune, network prune, container prune.
    pub const ORDER: [PruneKind; 4] = [
        PruneKind::BuildCache,
        PruneKind::Images,
        PruneKind::Networks,
        PruneKind::Containers,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PruneKind::BuildCache => "build_cache",
            PruneKind::Images => "images",
            PruneKind::Networks => "networks",
            PruneKind::Containers => "containers",
        }
    }
}

/// One thing a prune would remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PruneCandidate {
    pub id: String,
    pub name: String,
    pub size: Option<u64>,
    #[serde(skip)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrunePreviewRaw {
    pub build_cache: Vec<PruneCandidate>,
    pub images: Vec<PruneCandidate>,
    pub networks: Vec<PruneCandidate>,
    pub containers: Vec<PruneCandidate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct PruneOutcome {
    pub deleted: u64,
    pub reclaimed_bytes: u64,
}

#[async_trait::async_trait]
pub trait DockerPort: Send + Sync + 'static {
    /// Daemon reachability plus version; the cheapest health probe.
    async fn ping(&self) -> Result<(), DockerError>;
    async fn info(&self) -> Result<DockerInfo, DockerError>;
    async fn disk_usage(&self) -> Result<DiskUsage, DockerError>;

    /// All containers (`all=true`), newest first.
    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, DockerError>;
    /// One-shot stats for a running container; `None` when unavailable.
    async fn container_stats(&self, id: &str) -> Result<Option<ContainerStats>, DockerError>;
    async fn inspect_container(&self, id: &str) -> Result<ContainerDetail, DockerError>;
    async fn start_container(&self, id: &str) -> Result<(), DockerError>;
    async fn stop_container(&self, id: &str, timeout_secs: u32) -> Result<(), DockerError>;
    async fn restart_container(&self, id: &str, timeout_secs: u32) -> Result<(), DockerError>;

    /// Last `tail` lines of stdout and stderr, with timestamps.
    async fn logs(&self, id: &str, tail: u32) -> Result<Vec<LogLine>, DockerError>;
    /// Follow new output from `since` (unix seconds); ends when the container stops or the stream is dropped.
    fn logs_follow(&self, id: &str, since: i64) -> BoxStream<Result<LogLine, DockerError>>;

    /// Create and attach a TTY exec running the shell-detection command (04 2.2).
    async fn exec_start(&self, id: &str, cols: u16, rows: u16) -> Result<ExecIo, DockerError>;
    async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> Result<(), DockerError>;
    async fn exec_inspect(&self, exec_id: &str) -> Result<ExecState, DockerError>;

    /// Read-only candidates for each prune kind.
    async fn prune_preview(&self) -> Result<PrunePreviewRaw, DockerError>;
    async fn prune(&self, kind: PruneKind) -> Result<PruneOutcome, DockerError>;
}
