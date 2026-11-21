//! Host side of Backups: disk space, advisory locks, and the Project kinds that need Docker
//! (docs/rewrite/04 section 7). The Yanto-DB backup needs only the data dir and
//! lives in `yanto_core::backup`.

use std::path::Path;

use crate::backup::BackupKind;
use crate::error::CoreError;

/// Releases the lock when dropped.
pub struct LockGuard(#[allow(dead_code)] Box<dyn std::any::Any + Send + Sync>);

impl LockGuard {
    pub fn new(inner: impl std::any::Any + Send + Sync) -> Self {
        Self(Box::new(inner))
    }
}

impl std::fmt::Debug for LockGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LockGuard")
    }
}

pub trait BackupHost: Send + Sync + 'static {
    /// Free bytes on the filesystem holding `path` (nearest existing ancestor); `None` if unknown.
    fn free_bytes(&self, path: &Path) -> Option<u64>;

    /// Try to take the advisory lock `RUN/<name>.lock`. `Ok(None)` means someone else holds it.
    fn try_lock(&self, name: &str) -> Result<Option<LockGuard>, CoreError>;
}

/// A declared database service (`project_databases`, 02 4.2). `user_env` and `password_env` are
/// names of variables inside the container, never values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbTarget {
    pub compose_service: String,
    pub database: Option<String>,
    pub user_env: Option<String>,
    pub password_env: Option<String>,
}

/// What a Project backup should capture.
#[derive(Debug, Clone)]
pub struct ProjectBackupRequest {
    pub kind: BackupKind,
    pub project_slug: String,
    /// Volume name for `project_volumes`, the declared service for the dump kinds.
    pub target: Option<String>,
    /// Set for the dump kinds.
    pub db: Option<DbTarget>,
    pub stop_project: bool,
}

/// Where a restore writes (04 7.6). The new-volume mode is the default and touches nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreMode {
    /// Create `name` and extract into it; the Project keeps running.
    NewVolume(String),
    /// Stop the Project, empty the volume, extract, start the Project again.
    InPlace,
}

#[derive(Debug, Clone)]
pub struct ProjectRestoreRequest {
    pub kind: BackupKind,
    pub project_slug: String,
    pub target: Option<String>,
    pub db: Option<DbTarget>,
    pub mode: RestoreMode,
}

/// Project volume archives and Postgres/MySQL dumps. The real implementation drives the `docker`
/// CLI (`yanto_host::DockerProjectBackups`); tests use `testing::backup::FakeProjectBackups`.
#[async_trait::async_trait]
pub trait ProjectBackupPort: Send + Sync + 'static {
    /// Write the archive or dump for `req` to `dest` (a `.partial` path the caller renames).
    async fn create(&self, req: &ProjectBackupRequest, dest: &Path) -> Result<(), CoreError>;

    /// Feed the archive or dump at `source` back (checksum already verified by the caller).
    async fn restore(&self, req: &ProjectRestoreRequest, source: &Path) -> Result<(), CoreError>;

    /// Named volumes of the Compose project `slug` (bind mounts are not volumes).
    async fn list_volumes(&self, slug: &str) -> Result<Vec<String>, CoreError>;
}

/// Answers every call `Unavailable`. For callers that never touch Docker.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableProjectBackups;

fn unavailable<T>() -> Result<T, CoreError> {
    Err(CoreError::new(
        crate::ErrorKind::Unavailable,
        "Project backups are not available here (no Docker access)",
    ))
}

#[async_trait::async_trait]
impl ProjectBackupPort for UnavailableProjectBackups {
    async fn create(&self, _: &ProjectBackupRequest, _: &Path) -> Result<(), CoreError> {
        unavailable()
    }
    async fn restore(&self, _: &ProjectRestoreRequest, _: &Path) -> Result<(), CoreError> {
        unavailable()
    }
    async fn list_volumes(&self, _: &str) -> Result<Vec<String>, CoreError> {
        unavailable()
    }
}
