//! Registry HTTP API v2 client port for the Hosted Registry (docs/rewrite/04 4.3).
//! The reqwest implementation lives in `yanto-host`; tests use
//! `testing::registry_api::FakeRegistryApi`.

use async_trait::async_trait;

use crate::secrets::Secret;

/// Where and as whom to call the registry (always `127.0.0.1`).
#[derive(Debug, Clone)]
pub struct RegistryEndpoint {
    pub port: u16,
    pub username: String,
    pub password: Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryApiError {
    /// Connection refused, timeout, DNS: the container is down.
    #[error("registry unreachable: {0}")]
    Unreachable(String),
    /// 401: the htpasswd file the container loaded does not know the internal user.
    #[error("registry rejected the internal credential")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    /// 405 on delete: `storage.delete.enabled` is off.
    #[error("deleting is disabled in the registry configuration")]
    DeleteDisabled,
    #[error("registry answered {status}: {message}")]
    Other { status: u16, message: String },
}

/// One page of a catalog or tag listing. `next` is the `last=` value of the `Link: rel="next"` header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    pub items: Vec<String>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestInfo {
    /// `Docker-Content-Digest`.
    pub digest: String,
    pub media_type: String,
    /// `Content-Length` of the manifest document.
    pub manifest_size: u64,
    /// The manifest JSON (absent for `HEAD`).
    pub body: Option<Vec<u8>>,
}

#[async_trait]
pub trait RegistryApiPort: Send + Sync + 'static {
    /// `GET /v2/` with basic auth: `Ok(())` on 200, [`RegistryApiError::Unauthorized`] on 401.
    async fn ping(&self, ep: &RegistryEndpoint) -> Result<(), RegistryApiError>;
    async fn catalog(
        &self,
        ep: &RegistryEndpoint,
        last: Option<&str>,
        n: u32,
    ) -> Result<Page, RegistryApiError>;
    async fn tags(
        &self,
        ep: &RegistryEndpoint,
        repo: &str,
        last: Option<&str>,
        n: u32,
    ) -> Result<Page, RegistryApiError>;
    /// `HEAD` (`head_only`) or `GET` of `/v2/<repo>/manifests/<reference>` with the manifest `Accept` types.
    async fn manifest(
        &self,
        ep: &RegistryEndpoint,
        repo: &str,
        reference: &str,
        head_only: bool,
    ) -> Result<ManifestInfo, RegistryApiError>;
    /// `DELETE /v2/<repo>/manifests/<digest>`.
    async fn delete_manifest(
        &self,
        ep: &RegistryEndpoint,
        repo: &str,
        digest: &str,
    ) -> Result<(), RegistryApiError>;
}

/// What the Hosted Registry service needs from the outside besides the shared docker and
/// command ports: the HTTP client and two tunables.
#[derive(Clone)]
pub struct RegistryDeps {
    pub api: std::sync::Arc<dyn RegistryApiPort>,
    /// bcrypt cost for registry user hashes (10 in production, 4 in tests).
    pub bcrypt_cost: u32,
    /// How long `enable`/restarts wait for the API to answer.
    pub startup_wait: std::time::Duration,
}

impl RegistryDeps {
    pub fn new(api: std::sync::Arc<dyn RegistryApiPort>) -> Self {
        Self {
            api,
            bcrypt_cost: crate::hosted_registry::BCRYPT_COST,
            startup_wait: std::time::Duration::from_secs(20),
        }
    }
}

impl std::fmt::Debug for RegistryDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RegistryDeps")
    }
}
