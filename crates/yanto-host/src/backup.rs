//! Disk space and advisory locks for Backups.

use std::path::{Path, PathBuf};

use yanto_core::CoreError;
use yanto_core::ports::{BackupHost, LockGuard};

#[derive(Debug, Clone)]
pub struct HostBackup {
    run_dir: PathBuf,
}

impl HostBackup {
    /// Locks live in `run_dir` (`/run/yanto`, created `0700` when missing).
    pub fn new(run_dir: impl Into<PathBuf>) -> Self {
        Self {
            run_dir: run_dir.into(),
        }
    }
}

impl BackupHost for HostBackup {
    fn free_bytes(&self, path: &Path) -> Option<u64> {
        let mut p = path;
        while !p.exists() {
            p = p.parent()?;
        }
        let st = nix::sys::statvfs::statvfs(p).ok()?;
        #[allow(clippy::useless_conversion, clippy::unnecessary_cast)]
        Some((st.blocks_available() as u64).saturating_mul(st.fragment_size() as u64))
    }

    fn try_lock(&self, name: &str) -> Result<Option<LockGuard>, CoreError> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(CoreError::validation("lock", "invalid lock name"));
        }
        crate::fsutil::ensure_private_dir(&self.run_dir)?;
        Ok(
            crate::fsutil::try_flock(&self.run_dir.join(format!("{name}.lock")))?
                .map(LockGuard::new),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_is_refused_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let h = HostBackup::new(dir.path().join("run"));
        let first = h.try_lock("backup-yanto").unwrap().expect("first lock");
        assert!(h.try_lock("backup-yanto").unwrap().is_none());
        assert!(h.try_lock("backup-other").unwrap().is_some());
        drop(first);
        assert!(h.try_lock("backup-yanto").unwrap().is_some());
        assert!(h.try_lock("../escape").is_err());
    }

    #[test]
    fn free_bytes_walks_up_to_an_existing_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let h = HostBackup::new(dir.path());
        assert!(h.free_bytes(&dir.path().join("a/b/c")).unwrap() > 0);
    }
}
