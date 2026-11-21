//! Fakes for [`BackupHost`] and [`ProjectBackupPort`].

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::error::CoreError;
use crate::ports::{
    BackupHost, LockGuard, ProjectBackupPort, ProjectBackupRequest, ProjectRestoreRequest,
};

#[derive(Debug)]
pub struct FakeBackupHost {
    free: Mutex<Option<u64>>,
    held: Arc<Mutex<HashSet<String>>>,
}

impl Default for FakeBackupHost {
    fn default() -> Self {
        Self {
            free: Mutex::new(Some(1 << 50)),
            held: Arc::default(),
        }
    }
}

impl FakeBackupHost {
    pub fn set_free_bytes(&self, v: Option<u64>) {
        *self.free.lock().expect("lock") = v;
    }
}

struct Held {
    set: Arc<Mutex<HashSet<String>>>,
    name: String,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.set.lock().expect("lock").remove(&self.name);
    }
}

impl BackupHost for FakeBackupHost {
    fn free_bytes(&self, _path: &Path) -> Option<u64> {
        *self.free.lock().expect("lock")
    }

    fn try_lock(&self, name: &str) -> Result<Option<LockGuard>, CoreError> {
        if !self.held.lock().expect("lock").insert(name.to_owned()) {
            return Ok(None);
        }
        Ok(Some(LockGuard::new(Held {
            set: self.held.clone(),
            name: name.to_owned(),
        })))
    }
}

/// Records requests; writes `fake-archive` to `dest` unless a failure is queued.
#[derive(Debug, Default)]
pub struct FakeProjectBackups {
    calls: Mutex<Vec<ProjectBackupRequest>>,
    restores: Mutex<Vec<(ProjectRestoreRequest, Vec<u8>)>>,
    volumes: Mutex<Vec<String>>,
    fail: Mutex<Option<CoreError>>,
}

impl FakeProjectBackups {
    pub fn calls(&self) -> Vec<ProjectBackupRequest> {
        self.calls.lock().expect("lock").clone()
    }

    /// Restore requests with the bytes of the source file at the time of the call.
    pub fn restores(&self) -> Vec<(ProjectRestoreRequest, Vec<u8>)> {
        self.restores.lock().expect("lock").clone()
    }

    pub fn set_volumes(&self, v: &[&str]) {
        *self.volumes.lock().expect("lock") = v.iter().map(|s| (*s).to_owned()).collect();
    }

    pub fn fail_next(&self, e: CoreError) {
        *self.fail.lock().expect("lock") = Some(e);
    }
}

#[async_trait::async_trait]
impl ProjectBackupPort for FakeProjectBackups {
    async fn create(&self, req: &ProjectBackupRequest, dest: &Path) -> Result<(), CoreError> {
        self.calls.lock().expect("lock").push(req.clone());
        if let Some(e) = self.fail.lock().expect("lock").take() {
            return Err(e);
        }
        std::fs::write(dest, b"fake-archive").map_err(|e| CoreError::internal("fake write", e))
    }

    async fn restore(&self, req: &ProjectRestoreRequest, source: &Path) -> Result<(), CoreError> {
        let bytes =
            std::fs::read(source).map_err(|e| CoreError::internal("fake restore read", e))?;
        self.restores
            .lock()
            .expect("lock")
            .push((req.clone(), bytes));
        match self.fail.lock().expect("lock").take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn list_volumes(&self, _slug: &str) -> Result<Vec<String>, CoreError> {
        Ok(self.volumes.lock().expect("lock").clone())
    }
}
