//! Read-only view of what is bound on the Host, for the port collision check.

use async_trait::async_trait;

use crate::error::CoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub port: u16,
    /// `tcp` or `udp`.
    pub protocol: String,
    /// Best-effort owner (`comm` of the process), when it could be resolved.
    pub process: Option<String>,
}

#[async_trait]
pub trait HostProbePort: Send + Sync + 'static {
    /// Every TCP/UDP socket currently listening on the Host.
    async fn listeners(&self) -> Result<Vec<Listener>, CoreError>;
}
