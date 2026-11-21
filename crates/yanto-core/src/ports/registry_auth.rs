//! Throwaway per-command `DOCKER_CONFIG` directories for registry pulls (04 4.7).
//! The directory lives on tmpfs (`/run/yanto`), holds only `auths`, and is removed
//! when the command finishes. `docker login` is never run.

use std::path::PathBuf;

use crate::error::CoreError;

/// One registry login, already decrypted. Never log or print it.
#[derive(Clone)]
pub struct AuthEntry {
    pub registry_key: String,
    pub username: String,
    pub secret: crate::secrets::Secret,
}

impl std::fmt::Debug for AuthEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthEntry")
            .field("registry_key", &self.registry_key)
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

pub trait RegistryAuthPort: Send + Sync + 'static {
    /// Create `<run>/docker-<scope>/` (0700) with a `config.json` (0600); returns the dir.
    fn create(&self, scope: &str, auths: &[AuthEntry]) -> Result<PathBuf, CoreError>;
    /// Remove the directory made by [`RegistryAuthPort::create`] (idempotent).
    fn remove(&self, scope: &str);
}
