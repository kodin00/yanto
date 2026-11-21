//! Host side of Backups: disk space, advisory locks, and (TODO(M2-merge)) the Project kinds that
//! need Docker (docs/rewrite/04 section 7). The Yanto-DB backup needs only the data dir and
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

/// What a Project backup should capture.
#[derive(Debug, Clone)]
pub struct ProjectBackupRequest {
    pub kind: BackupKind,
    pub project_slug: String,
    /// Declared database service for the dump kinds, volume name for `project_volumes`.
    pub target: Option<String>,
    pub stop_project: bool,
}

/// Project volume archives and Postgres/MySQL dumps. TODO(M2-merge): the real implementation needs
/// Projects, `project_databases` and the Docker port from M2; until then
/// [`crate::ports::UnavailableProjectBackups`] answers every call.
#[async_trait::async_trait]
pub trait ProjectBackupPort: Send + Sync + 'static {
    /// Write the archive or dump for `req` to `dest` (a `.partial` path the caller renames).
    async fn create(&self, req: &ProjectBackupRequest, dest: &Path) -> Result<(), CoreError>;
}

/// Placeholder until M2 merges: every Project backup is `Unavailable`.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableProjectBackups;

#[async_trait::async_trait]
impl ProjectBackupPort for UnavailableProjectBackups {
    async fn create(&self, req: &ProjectBackupRequest, _dest: &Path) -> Result<(), CoreError> {
        Err(CoreError::new(
            crate::ErrorKind::Unavailable,
            format!(
                "{} backups need Projects, which are not available in this build yet",
                req.kind.as_str()
            ),
        ))
    }
}
