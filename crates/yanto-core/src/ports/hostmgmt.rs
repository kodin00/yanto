//! The M5 ports bundled into one value so `ServiceDeps` carries a single field for them.

use std::sync::Arc;

use super::{
    AuthorizedKeysPort, BackupHost, CronExec, CronPort, HostUsers, ProjectBackupPort,
    UnavailableProjectBackups,
};

#[derive(Clone)]
pub struct HostMgmtPorts {
    pub users: Arc<dyn HostUsers>,
    pub cron: Arc<dyn CronPort>,
    pub cron_exec: Arc<dyn CronExec>,
    pub authorized_keys: Arc<dyn AuthorizedKeysPort>,
    pub backup_host: Arc<dyn BackupHost>,
    pub project_backups: Arc<dyn ProjectBackupPort>,
}

impl std::fmt::Debug for HostMgmtPorts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostMgmtPorts").finish_non_exhaustive()
    }
}

/// Every host operation fails with `Unavailable`. For callers that never touch the Host.
#[derive(Debug, Default, Clone, Copy)]
pub struct Unsupported;

fn unsupported<T>() -> Result<T, crate::CoreError> {
    Err(crate::CoreError::new(
        crate::ErrorKind::Unavailable,
        "host management is not available here",
    ))
}

impl HostUsers for Unsupported {
    fn lookup(&self, _: &str) -> Result<Option<super::UnixUser>, crate::CoreError> {
        unsupported()
    }
    fn login_users(&self) -> Result<Vec<super::UnixUser>, crate::CoreError> {
        unsupported()
    }
}

impl CronPort for Unsupported {
    fn yanto_binary(&self) -> Result<String, crate::CoreError> {
        unsupported()
    }
    fn read_managed(&self) -> Result<super::ManagedFile, crate::CoreError> {
        unsupported()
    }
    fn write_managed(&self, _: &str, _: bool) -> Result<(), crate::CoreError> {
        unsupported()
    }
    fn system_sources(&self) -> Result<Vec<super::CronSource>, crate::CoreError> {
        unsupported()
    }
    fn service_active(&self) -> Option<bool> {
        None
    }
}

impl CronExec for Unsupported {
    fn run(&self, _: &super::ExecSpec) -> Result<super::ExecOutcome, crate::CoreError> {
        unsupported()
    }
}

impl AuthorizedKeysPort for Unsupported {
    fn resolve(&self, _: &str) -> Result<super::UnixUser, crate::CoreError> {
        unsupported()
    }
    fn file_path(&self, user: &super::UnixUser) -> std::path::PathBuf {
        user.home.join(".ssh/authorized_keys")
    }
    fn read(&self, _: &str) -> Result<Option<Vec<u8>>, crate::CoreError> {
        unsupported()
    }
    fn update(
        &self,
        _: &str,
        _: &mut dyn FnMut(&[u8]) -> Result<Option<super::KeysEdit>, crate::CoreError>,
    ) -> Result<super::KeysUpdated, crate::CoreError> {
        unsupported()
    }
    fn sshd_info(&self) -> Option<super::SshdInfo> {
        None
    }
}

impl BackupHost for Unsupported {
    fn free_bytes(&self, _: &std::path::Path) -> Option<u64> {
        None
    }
    fn try_lock(&self, _: &str) -> Result<Option<super::LockGuard>, crate::CoreError> {
        Ok(Some(super::LockGuard::new(())))
    }
}

impl HostMgmtPorts {
    /// All ports `Unsupported` (Project backups use [`UnavailableProjectBackups`]).
    pub fn unsupported() -> Self {
        let u = Arc::new(Unsupported);
        Self {
            users: u.clone(),
            cron: u.clone(),
            cron_exec: u.clone(),
            authorized_keys: u.clone(),
            backup_host: u,
            project_backups: Arc::new(UnavailableProjectBackups),
        }
    }
}
