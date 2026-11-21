//! Ports for the M7 Host Daemons, Cloudflare API, Traefik files and the `frps` files and dashboard.
//! Real implementations live in `yanto-host`; fakes in `crate::testing::ingress`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::CoreError;
use crate::secrets::Secret;

// ---- systemd ------------------------------------------------------------------------------

/// `systemctl show <unit> -p LoadState,ActiveState,SubState,FragmentPath,MainPID,ExecStart`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitProps {
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub fragment_path: Option<String>,
    pub main_pid: Option<u32>,
    /// Raw `ExecStart=` value (systemd prints `{ path=... ; argv[]=... ; ... }`).
    pub exec_start: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitAction {
    Start,
    Stop,
    Restart,
}

impl UnitAction {
    pub fn as_str(self) -> &'static str {
        match self {
            UnitAction::Start => "start",
            UnitAction::Stop => "stop",
            UnitAction::Restart => "restart",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionResult {
    pub ok: bool,
    pub output: String,
}

#[async_trait]
pub trait SystemdPort: Send + Sync + 'static {
    /// `Err` only when `systemctl` itself cannot run; a missing unit is `load_state = "not-found"`.
    async fn show(&self, unit: &str) -> Result<UnitProps, CoreError>;
    async fn control(&self, unit: &str, action: UnitAction) -> Result<ActionResult, CoreError>;
    /// `systemctl status <unit> --no-pager -n 20`.
    async fn status_text(&self, unit: &str) -> Result<String, CoreError>;
    /// `journalctl -u <unit> -n <lines> --no-pager`.
    async fn journal_tail(&self, unit: &str, lines: u32) -> Result<String, CoreError>;
    /// Run `<program> <args>` and return the first non-empty stdout line; `None` when the
    /// program is missing or exits non-zero.
    async fn binary_version(&self, program: &str, args: &[&str]) -> Option<String>;
}

// ---- Cloudflare ---------------------------------------------------------------------------

/// A failed Cloudflare API call: the HTTP status and the API's own message (never the token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfError {
    /// `0` when the request never got an HTTP answer (network error, timeout).
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for CfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status == 0 {
            write!(f, "Cloudflare API unreachable: {}", self.message)
        } else {
            write!(
                f,
                "Cloudflare API error (HTTP {}): {}",
                self.status, self.message
            )
        }
    }
}

impl std::error::Error for CfError {}

pub type CfResult<T> = Result<T, CfError>;

/// Credentials for one call. The token is exposed only inside the HTTP adapter.
#[derive(Clone, Copy)]
pub struct CfAuth<'a> {
    pub account_id: &'a str,
    pub token: &'a Secret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfZone {
    pub id: String,
    pub name: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfTunnel {
    pub id: String,
    pub name: String,
    /// Raw API status (`healthy`, `degraded`, `down`, `inactive` ...; allowed values unverified).
    pub status: String,
}

/// One cloudflared ingress rule; the final catch-all has no hostname.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressRule {
    pub hostname: Option<String>,
    pub service: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfDnsRecord {
    pub id: String,
    pub record_type: String,
    pub name: String,
    pub content: String,
    pub ttl: i64,
    pub proxied: bool,
    pub priority: Option<i64>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsRecordInput {
    pub record_type: String,
    pub name: String,
    pub content: String,
    pub ttl: i64,
    pub proxied: bool,
    pub priority: Option<i64>,
    pub comment: Option<String>,
}

#[async_trait]
pub trait CloudflarePort: Send + Sync + 'static {
    /// `GET /accounts/{id}/cfd_tunnel?per_page=1`: the token can reach the account's tunnels.
    async fn check_tunnels(&self, auth: CfAuth<'_>) -> CfResult<()>;
    /// `GET /zones?account.id={id}&per_page=50`.
    async fn list_zones(&self, auth: CfAuth<'_>) -> CfResult<Vec<CfZone>>;
    /// `POST /accounts/{id}/cfd_tunnel` with `config_src = cloudflare`.
    async fn create_tunnel(&self, auth: CfAuth<'_>, name: &str) -> CfResult<CfTunnel>;
    async fn get_tunnel(&self, auth: CfAuth<'_>, tunnel_id: &str) -> CfResult<CfTunnel>;
    /// `GET .../cfd_tunnel/{id}/token`: the connector token (never stored).
    async fn tunnel_token(&self, auth: CfAuth<'_>, tunnel_id: &str) -> CfResult<Secret>;
    async fn put_ingress(
        &self,
        auth: CfAuth<'_>,
        tunnel_id: &str,
        rules: &[IngressRule],
    ) -> CfResult<()>;
    async fn get_ingress(&self, auth: CfAuth<'_>, tunnel_id: &str) -> CfResult<Vec<IngressRule>>;
    async fn delete_tunnel(&self, auth: CfAuth<'_>, tunnel_id: &str) -> CfResult<()>;
    /// Records of a zone, optionally filtered by exact type and name.
    async fn list_dns(
        &self,
        token: &Secret,
        zone_id: &str,
        record_type: Option<&str>,
        name: Option<&str>,
    ) -> CfResult<Vec<CfDnsRecord>>;
    async fn create_dns(
        &self,
        token: &Secret,
        zone_id: &str,
        rec: &DnsRecordInput,
    ) -> CfResult<CfDnsRecord>;
    async fn update_dns(
        &self,
        token: &Secret,
        zone_id: &str,
        record_id: &str,
        rec: &DnsRecordInput,
    ) -> CfResult<CfDnsRecord>;
    async fn delete_dns(&self, token: &Secret, zone_id: &str, record_id: &str) -> CfResult<()>;
}

// ---- Traefik ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirStatus {
    pub exists: bool,
    pub writable: bool,
}

/// The one dynamic file Yanto owns, plus read access to the operator's static config.
pub trait TraefikFilesPort: Send + Sync + 'static {
    fn dynamic_dir(&self) -> PathBuf;
    fn dynamic_file(&self) -> PathBuf;
    fn dir_status(&self) -> DirStatus;
    /// Current content of `yanto.yml`; `None` when absent.
    fn read_dynamic(&self) -> Result<Option<String>, CoreError>;
    /// Atomic write: temp file in the *parent* of the dynamic dir, fsync, rename, fsync dir. The
    /// previous content is kept as `<dir>/../yanto.yml.bak`.
    fn write_dynamic(&self, content: &str) -> Result<(), CoreError>;
    /// Content of the operator's static config; `None` when the file does not exist.
    fn read_static(&self, path: &std::path::Path) -> Result<Option<String>, CoreError>;
}

// ---- frps ---------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrpFile {
    pub content: String,
    /// Unix mode bits, for the world-readable warning.
    pub mode: u32,
}

pub trait FrpFilesPort: Send + Sync + 'static {
    fn read(&self, path: &std::path::Path) -> Result<Option<FrpFile>, CoreError>;
    /// Backup of the old content to `<path>.yanto.bak` (0600), then temp + fsync + rename keeping
    /// the original mode and owner.
    fn write(&self, path: &std::path::Path, content: &str) -> Result<(), CoreError>;
    fn read_backup(&self, path: &std::path::Path) -> Result<Option<String>, CoreError>;
}

#[derive(Debug, Clone)]
pub struct DashboardTarget {
    pub addr: String,
    pub port: u16,
    pub auth: Option<(String, Secret)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpAnswer {
    pub status: u16,
    pub body: String,
}

#[async_trait]
pub trait FrpDashboardPort: Send + Sync + 'static {
    /// `GET http://<addr>:<port><path>` with Basic auth when set; 2 s timeout. `Err` = unreachable.
    async fn get(&self, target: &DashboardTarget, path: &str) -> Result<HttpAnswer, CoreError>;
}

// ---- local probes -------------------------------------------------------------------------

#[async_trait]
pub trait IngressProbePort: Send + Sync + 'static {
    /// TCP connect to `127.0.0.1:<port>` (1 s).
    async fn tcp_connect(&self, port: u16) -> bool;
    /// `GET http://127.0.0.1:<port>/` with the given `Host` header; the status code only.
    async fn local_http(&self, port: u16, host: &str, timeout_secs: u64) -> Result<u16, String>;
    /// Poll `GET http://127.0.0.1:<port>/` with `Host: <host>` until the status is not 404 or
    /// `wait_secs` pass; the last status (or error text). Used after a Traefik write.
    async fn await_route(&self, port: u16, host: &str, wait_secs: u64) -> Result<u16, String>;
    /// `HEAD https://<hostname>/` (5 s); the status code, or the TLS/connect error text.
    async fn public_head(&self, hostname: &str) -> Result<u16, String>;
}

// ---- hosted registry hook (M6) ------------------------------------------------------------

/// What a Hostname with `target_kind = registry` points at. Production uses
/// `service::registry::SettingsRegistryTarget` (the M6 registry settings), wired in
/// `ServiceDeps::ingress`.
#[async_trait]
pub trait RegistryTarget: Send + Sync + 'static {
    /// The loopback listen port of the Hosted Registry when it is enabled, else `None`.
    async fn listen_port(&self) -> Result<Option<u16>, CoreError>;
}

/// No Hosted Registry: registry Hostnames are refused (harnesses that do not register one).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoRegistry;

#[async_trait]
impl RegistryTarget for NoRegistry {
    async fn listen_port(&self) -> Result<Option<u16>, CoreError> {
        Ok(None)
    }
}

/// All M7 ports in one value so `ServiceDeps` carries a single field.
#[derive(Clone)]
pub struct IngressPorts {
    pub systemd: Arc<dyn SystemdPort>,
    pub cloudflare: Arc<dyn CloudflarePort>,
    pub traefik: Arc<dyn TraefikFilesPort>,
    pub frp_files: Arc<dyn FrpFilesPort>,
    pub frp_dashboard: Arc<dyn FrpDashboardPort>,
    pub probe: Arc<dyn IngressProbePort>,
    pub registry: Arc<dyn RegistryTarget>,
}

impl std::fmt::Debug for IngressPorts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IngressPorts").finish_non_exhaustive()
    }
}
