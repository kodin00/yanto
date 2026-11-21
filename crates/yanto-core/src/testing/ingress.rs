//! Fakes for the M7 ports: systemd, Cloudflare, Traefik files, `frps` files and dashboard,
//! local probes and the registry target hook. Each records calls and returns scripted results.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::db::Db;
use crate::error::CoreError;
use crate::ports::*;
use crate::secrets::Secret;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().expect("fake mutex")
}

// ---- systemd ------------------------------------------------------------------------------

#[derive(Default)]
pub struct FakeSystemd {
    units: Mutex<BTreeMap<String, UnitProps>>,
    pub calls: Mutex<Vec<String>>,
    versions: Mutex<BTreeMap<String, String>>,
    fail_control: Mutex<Option<String>>,
    pub status_output: Mutex<String>,
    pub journal_output: Mutex<String>,
}

impl FakeSystemd {
    /// Install a unit (`LoadState=loaded`) with the given state and `ExecStart` argv.
    pub fn install(&self, unit: &str, active: bool, exec_argv: &str) {
        let bin = exec_argv.split_whitespace().next().unwrap_or("").to_owned();
        lock(&self.units).insert(
            unit.to_owned(),
            UnitProps {
                load_state: "loaded".into(),
                active_state: if active { "active" } else { "inactive" }.into(),
                sub_state: if active { "running" } else { "dead" }.into(),
                fragment_path: Some(format!("/etc/systemd/system/{unit}.service")),
                main_pid: active.then_some(4242),
                exec_start: format!(
                    "{{ path={bin} ; argv[]={exec_argv} ; ignore_errors=no ; start_time=[n/a] }}"
                ),
            },
        );
    }

    pub fn set_version(&self, program: &str, version: &str) {
        lock(&self.versions).insert(program.to_owned(), version.to_owned());
    }

    pub fn is_active(&self, unit: &str) -> bool {
        lock(&self.units)
            .get(unit)
            .is_some_and(|u| u.active_state == "active")
    }

    /// The next `control` call exits non-zero with this output.
    pub fn fail_control(&self, output: &str) {
        *lock(&self.fail_control) = Some(output.to_owned());
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }
}

#[async_trait]
impl SystemdPort for FakeSystemd {
    async fn show(&self, unit: &str) -> Result<UnitProps, CoreError> {
        lock(&self.calls).push(format!("show {unit}"));
        Ok(lock(&self.units).get(unit).cloned().unwrap_or(UnitProps {
            load_state: "not-found".into(),
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            ..Default::default()
        }))
    }

    async fn control(&self, unit: &str, action: UnitAction) -> Result<ActionResult, CoreError> {
        lock(&self.calls).push(format!("{} {unit}", action.as_str()));
        if let Some(out) = lock(&self.fail_control).take() {
            return Ok(ActionResult {
                ok: false,
                output: out,
            });
        }
        let mut units = lock(&self.units);
        let Some(u) = units.get_mut(unit) else {
            return Ok(ActionResult {
                ok: false,
                output: format!("Unit {unit}.service not found."),
            });
        };
        let active = !matches!(action, UnitAction::Stop);
        u.active_state = if active { "active" } else { "inactive" }.into();
        u.sub_state = if active { "running" } else { "dead" }.into();
        Ok(ActionResult {
            ok: true,
            output: String::new(),
        })
    }

    async fn status_text(&self, unit: &str) -> Result<String, CoreError> {
        lock(&self.calls).push(format!("status {unit}"));
        Ok(lock(&self.status_output).clone())
    }

    async fn journal_tail(&self, unit: &str, lines: u32) -> Result<String, CoreError> {
        lock(&self.calls).push(format!("journal {unit} {lines}"));
        Ok(lock(&self.journal_output).clone())
    }

    async fn binary_version(&self, program: &str, _args: &[&str]) -> Option<String> {
        lock(&self.versions).get(program).cloned()
    }
}

// ---- Cloudflare ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeTunnel {
    pub name: String,
    pub status: String,
    pub ingress: Vec<IngressRule>,
}

#[derive(Default)]
struct CfState {
    valid_token: Option<String>,
    zones: Vec<CfZone>,
    tunnels: BTreeMap<String, FakeTunnel>,
    dns: BTreeMap<String, Vec<CfDnsRecord>>,
    next: u32,
    fail_next: Option<CfError>,
    fail_ingress_put: Option<CfError>,
}

#[derive(Default)]
pub struct FakeCloudflare {
    st: Mutex<CfState>,
    pub calls: Mutex<Vec<String>>,
}

impl FakeCloudflare {
    /// Only this token is accepted; others get HTTP 403 (so tests cover the bad-token path).
    pub fn accept_token(&self, token: &str) {
        lock(&self.st).valid_token = Some(token.to_owned());
    }

    pub fn add_zone(&self, id: &str, name: &str) {
        lock(&self.st).zones.push(CfZone {
            id: id.into(),
            name: name.into(),
            status: Some("active".into()),
        });
    }

    pub fn fail_next(&self, status: u16, message: &str) {
        lock(&self.st).fail_next = Some(CfError {
            status,
            message: message.into(),
        });
    }

    pub fn fail_ingress_put(&self, status: u16, message: &str) {
        lock(&self.st).fail_ingress_put = Some(CfError {
            status,
            message: message.into(),
        });
    }

    pub fn clear_ingress_failure(&self) {
        lock(&self.st).fail_ingress_put = None;
    }

    pub fn tunnel(&self, id: &str) -> Option<FakeTunnel> {
        lock(&self.st).tunnels.get(id).cloned()
    }

    pub fn set_tunnel_status(&self, id: &str, status: &str) {
        if let Some(t) = lock(&self.st).tunnels.get_mut(id) {
            t.status = status.into();
        }
    }

    /// Simulate a manual edit in the Cloudflare dashboard.
    pub fn set_remote_ingress(&self, id: &str, rules: Vec<IngressRule>) {
        if let Some(t) = lock(&self.st).tunnels.get_mut(id) {
            t.ingress = rules;
        }
    }

    pub fn dns(&self, zone: &str) -> Vec<CfDnsRecord> {
        lock(&self.st).dns.get(zone).cloned().unwrap_or_default()
    }

    pub fn seed_dns(&self, zone: &str, rec: CfDnsRecord) {
        lock(&self.st).dns.entry(zone.into()).or_default().push(rec);
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }

    fn enter(&self, what: &str, token: &Secret) -> CfResult<()> {
        lock(&self.calls).push(what.to_owned());
        let mut st = lock(&self.st);
        if let Some(e) = st.fail_next.take() {
            return Err(e);
        }
        match &st.valid_token {
            Some(t) if t != token.expose() => Err(CfError {
                status: 403,
                message: "Invalid API Token".into(),
            }),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl CloudflarePort for FakeCloudflare {
    async fn check_tunnels(&self, a: CfAuth<'_>) -> CfResult<()> {
        self.enter("check_tunnels", a.token)
    }

    async fn list_zones(&self, a: CfAuth<'_>) -> CfResult<Vec<CfZone>> {
        self.enter("list_zones", a.token)?;
        Ok(lock(&self.st).zones.clone())
    }

    async fn create_tunnel(&self, a: CfAuth<'_>, name: &str) -> CfResult<CfTunnel> {
        self.enter("create_tunnel", a.token)?;
        let mut st = lock(&self.st);
        st.next += 1;
        let id = format!("tun-{:04}", st.next);
        st.tunnels.insert(
            id.clone(),
            FakeTunnel {
                name: name.into(),
                status: "inactive".into(),
                ingress: Vec::new(),
            },
        );
        Ok(CfTunnel {
            id,
            name: name.into(),
            status: "inactive".into(),
        })
    }

    async fn get_tunnel(&self, a: CfAuth<'_>, id: &str) -> CfResult<CfTunnel> {
        self.enter("get_tunnel", a.token)?;
        lock(&self.st)
            .tunnels
            .get(id)
            .map(|t| CfTunnel {
                id: id.into(),
                name: t.name.clone(),
                status: t.status.clone(),
            })
            .ok_or(CfError {
                status: 404,
                message: "tunnel not found".into(),
            })
    }

    async fn tunnel_token(&self, a: CfAuth<'_>, id: &str) -> CfResult<Secret> {
        self.enter("tunnel_token", a.token)?;
        Ok(Secret::new(format!("fake-connector-token-{id}")))
    }

    async fn put_ingress(&self, a: CfAuth<'_>, id: &str, rules: &[IngressRule]) -> CfResult<()> {
        self.enter("put_ingress", a.token)?;
        let mut st = lock(&self.st);
        if let Some(e) = st.fail_ingress_put.clone() {
            return Err(e);
        }
        match st.tunnels.get_mut(id) {
            Some(t) => {
                t.ingress = rules.to_vec();
                Ok(())
            }
            None => Err(CfError {
                status: 404,
                message: "tunnel not found".into(),
            }),
        }
    }

    async fn get_ingress(&self, a: CfAuth<'_>, id: &str) -> CfResult<Vec<IngressRule>> {
        self.enter("get_ingress", a.token)?;
        lock(&self.st)
            .tunnels
            .get(id)
            .map(|t| t.ingress.clone())
            .ok_or(CfError {
                status: 404,
                message: "tunnel not found".into(),
            })
    }

    async fn delete_tunnel(&self, a: CfAuth<'_>, id: &str) -> CfResult<()> {
        self.enter("delete_tunnel", a.token)?;
        lock(&self.st).tunnels.remove(id);
        Ok(())
    }

    async fn list_dns(
        &self,
        token: &Secret,
        zone: &str,
        ty: Option<&str>,
        name: Option<&str>,
    ) -> CfResult<Vec<CfDnsRecord>> {
        self.enter("list_dns", token)?;
        Ok(lock(&self.st)
            .dns
            .get(zone)
            .map(|v| {
                v.iter()
                    .filter(|r| ty.is_none_or(|t| r.record_type == t))
                    .filter(|r| name.is_none_or(|n| r.name == n))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn create_dns(
        &self,
        token: &Secret,
        zone: &str,
        rec: &DnsRecordInput,
    ) -> CfResult<CfDnsRecord> {
        self.enter("create_dns", token)?;
        let mut st = lock(&self.st);
        st.next += 1;
        let r = CfDnsRecord {
            id: format!("rec-{:04}", st.next),
            record_type: rec.record_type.clone(),
            name: rec.name.clone(),
            content: rec.content.clone(),
            ttl: rec.ttl,
            proxied: rec.proxied,
            priority: rec.priority,
            comment: rec.comment.clone(),
        };
        st.dns.entry(zone.into()).or_default().push(r.clone());
        Ok(r)
    }

    async fn update_dns(
        &self,
        token: &Secret,
        zone: &str,
        id: &str,
        rec: &DnsRecordInput,
    ) -> CfResult<CfDnsRecord> {
        self.enter("update_dns", token)?;
        let mut st = lock(&self.st);
        let slot = st
            .dns
            .get_mut(zone)
            .and_then(|v| v.iter_mut().find(|r| r.id == id))
            .ok_or(CfError {
                status: 404,
                message: "record not found".into(),
            })?;
        *slot = CfDnsRecord {
            id: id.into(),
            record_type: rec.record_type.clone(),
            name: rec.name.clone(),
            content: rec.content.clone(),
            ttl: rec.ttl,
            proxied: rec.proxied,
            priority: rec.priority,
            comment: rec.comment.clone(),
        };
        Ok(slot.clone())
    }

    async fn delete_dns(&self, token: &Secret, zone: &str, id: &str) -> CfResult<()> {
        self.enter("delete_dns", token)?;
        if let Some(v) = lock(&self.st).dns.get_mut(zone) {
            v.retain(|r| r.id != id);
        }
        Ok(())
    }
}

// ---- Traefik ------------------------------------------------------------------------------

pub struct FakeTraefik {
    pub dir: PathBuf,
    pub content: Mutex<Option<String>>,
    pub writes: Mutex<Vec<String>>,
    pub status: Mutex<DirStatus>,
    pub static_files: Mutex<BTreeMap<PathBuf, String>>,
    fail_write: Mutex<bool>,
}

impl FakeTraefik {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            content: Mutex::new(None),
            writes: Mutex::default(),
            status: Mutex::new(DirStatus {
                exists: true,
                writable: true,
            }),
            static_files: Mutex::default(),
            fail_write: Mutex::new(false),
        }
    }

    pub fn set_static(&self, path: &Path, text: &str) {
        lock(&self.static_files).insert(path.to_path_buf(), text.to_owned());
    }

    pub fn set_dir_status(&self, exists: bool, writable: bool) {
        *lock(&self.status) = DirStatus { exists, writable };
    }

    pub fn fail_write(&self) {
        *lock(&self.fail_write) = true;
    }

    pub fn current(&self) -> Option<String> {
        lock(&self.content).clone()
    }

    pub fn write_count(&self) -> usize {
        lock(&self.writes).len()
    }
}

impl TraefikFilesPort for FakeTraefik {
    fn dynamic_dir(&self) -> PathBuf {
        self.dir.clone()
    }
    fn dynamic_file(&self) -> PathBuf {
        self.dir.join("yanto.yml")
    }
    fn dir_status(&self) -> DirStatus {
        *lock(&self.status)
    }
    fn read_dynamic(&self) -> Result<Option<String>, CoreError> {
        Ok(lock(&self.content).clone())
    }
    fn write_dynamic(&self, content: &str) -> Result<(), CoreError> {
        if std::mem::take(&mut *lock(&self.fail_write)) {
            return Err(CoreError::new(
                crate::ErrorKind::Unavailable,
                format!(
                    "cannot write {}: permission denied",
                    self.dynamic_file().display()
                ),
            ));
        }
        lock(&self.writes).push(content.to_owned());
        *lock(&self.content) = Some(content.to_owned());
        Ok(())
    }
    fn read_static(&self, path: &Path) -> Result<Option<String>, CoreError> {
        Ok(lock(&self.static_files).get(path).cloned())
    }
}

// ---- frps ---------------------------------------------------------------------------------

#[derive(Default)]
pub struct FakeFrpFiles {
    pub files: Mutex<BTreeMap<PathBuf, FrpFile>>,
    pub backups: Mutex<BTreeMap<PathBuf, String>>,
    pub writes: Mutex<u32>,
}

impl FakeFrpFiles {
    pub fn put(&self, path: &Path, content: &str, mode: u32) {
        lock(&self.files).insert(
            path.to_path_buf(),
            FrpFile {
                content: content.to_owned(),
                mode,
            },
        );
    }

    pub fn content(&self, path: &Path) -> Option<String> {
        lock(&self.files).get(path).map(|f| f.content.clone())
    }

    pub fn backup(&self, path: &Path) -> Option<String> {
        lock(&self.backups).get(path).cloned()
    }
}

impl FrpFilesPort for FakeFrpFiles {
    fn read(&self, path: &Path) -> Result<Option<FrpFile>, CoreError> {
        Ok(lock(&self.files).get(path).cloned())
    }
    fn write(&self, path: &Path, content: &str) -> Result<(), CoreError> {
        let mut files = lock(&self.files);
        let mode = files.get(path).map_or(0o600, |f| f.mode);
        if let Some(old) = files.get(path) {
            lock(&self.backups).insert(path.to_path_buf(), old.content.clone());
        }
        files.insert(
            path.to_path_buf(),
            FrpFile {
                content: content.to_owned(),
                mode,
            },
        );
        *lock(&self.writes) += 1;
        Ok(())
    }
    fn read_backup(&self, path: &Path) -> Result<Option<String>, CoreError> {
        Ok(lock(&self.backups).get(path).cloned())
    }
}

#[derive(Default)]
pub struct FakeFrpDashboard {
    answers: Mutex<BTreeMap<String, HttpAnswer>>,
    pub requests: Mutex<Vec<(DashboardTarget, String)>>,
    unreachable: Mutex<bool>,
}

impl FakeFrpDashboard {
    pub fn answer(&self, path: &str, status: u16, body: &str) {
        lock(&self.answers).insert(
            path.to_owned(),
            HttpAnswer {
                status,
                body: body.to_owned(),
            },
        );
    }

    pub fn set_unreachable(&self, v: bool) {
        *lock(&self.unreachable) = v;
    }
}

#[async_trait]
impl FrpDashboardPort for FakeFrpDashboard {
    async fn get(&self, target: &DashboardTarget, path: &str) -> Result<HttpAnswer, CoreError> {
        lock(&self.requests).push((target.clone(), path.to_owned()));
        if *lock(&self.unreachable) {
            return Err(CoreError::new(
                crate::ErrorKind::Unavailable,
                "dashboard unreachable",
            ));
        }
        Ok(lock(&self.answers)
            .get(path)
            .cloned()
            .unwrap_or(HttpAnswer {
                status: 404,
                body: String::new(),
            }))
    }
}

// ---- probes and registry ------------------------------------------------------------------

#[derive(Default)]
pub struct FakeIngressProbe {
    open: Mutex<BTreeSet<u16>>,
    http: Mutex<BTreeMap<(u16, String), Result<u16, String>>>,
    public: Mutex<BTreeMap<String, Result<u16, String>>>,
}

impl FakeIngressProbe {
    pub fn open_port(&self, port: u16) {
        lock(&self.open).insert(port);
    }

    pub fn http_status(&self, port: u16, host: &str, status: u16) {
        lock(&self.http).insert((port, host.to_owned()), Ok(status));
    }

    pub fn public_status(&self, host: &str, r: Result<u16, String>) {
        lock(&self.public).insert(host.to_owned(), r);
    }
}

#[async_trait]
impl IngressProbePort for FakeIngressProbe {
    async fn tcp_connect(&self, port: u16) -> bool {
        lock(&self.open).contains(&port)
    }
    async fn local_http(&self, port: u16, host: &str, _t: u64) -> Result<u16, String> {
        lock(&self.http)
            .get(&(port, host.to_owned()))
            .cloned()
            .unwrap_or(Err("connection refused".into()))
    }
    async fn await_route(&self, port: u16, host: &str, _wait: u64) -> Result<u16, String> {
        self.local_http(port, host, 0).await
    }
    async fn public_head(&self, hostname: &str) -> Result<u16, String> {
        lock(&self.public)
            .get(hostname)
            .cloned()
            .unwrap_or(Err("no route".into()))
    }
}

/// Registry hook for tests: `enable(port)` forces `Some(port)`; otherwise it answers from the
/// stored Hosted Registry settings, so enabling the registry through its service is seen too.
pub struct FakeRegistryTarget {
    forced: Mutex<Option<u16>>,
    stored: Arc<dyn RegistryTarget>,
}

impl FakeRegistryTarget {
    pub fn new(db: Db) -> Self {
        Self {
            forced: Mutex::new(None),
            stored: Arc::new(crate::service::registry::SettingsRegistryTarget::new(db)),
        }
    }

    pub fn enable(&self, port: u16) {
        *lock(&self.forced) = Some(port);
    }
}

#[async_trait]
impl RegistryTarget for FakeRegistryTarget {
    async fn listen_port(&self) -> Result<Option<u16>, CoreError> {
        let forced = *lock(&self.forced);
        match forced {
            Some(p) => Ok(Some(p)),
            None => self.stored.listen_port().await,
        }
    }
}

/// All M7 fakes wired together (`TestCore::ingress`).
pub struct IngressFakes {
    pub systemd: Arc<FakeSystemd>,
    pub cloudflare: Arc<FakeCloudflare>,
    pub traefik: Arc<FakeTraefik>,
    pub frp_files: Arc<FakeFrpFiles>,
    pub frp_dashboard: Arc<FakeFrpDashboard>,
    pub probe: Arc<FakeIngressProbe>,
    pub registry: Arc<FakeRegistryTarget>,
}

impl IngressFakes {
    pub fn new(dynamic_dir: &Path, db: &Db) -> Self {
        Self {
            systemd: Arc::default(),
            cloudflare: Arc::default(),
            traefik: Arc::new(FakeTraefik::new(dynamic_dir)),
            frp_files: Arc::default(),
            frp_dashboard: Arc::default(),
            probe: Arc::default(),
            registry: Arc::new(FakeRegistryTarget::new(db.clone())),
        }
    }

    pub fn ports(&self) -> IngressPorts {
        IngressPorts {
            systemd: self.systemd.clone(),
            cloudflare: self.cloudflare.clone(),
            traefik: self.traefik.clone(),
            frp_files: self.frp_files.clone(),
            frp_dashboard: self.frp_dashboard.clone(),
            probe: self.probe.clone(),
            registry: self.registry.clone(),
        }
    }
}

// ---- TestCore fixtures --------------------------------------------------------------------

/// Cloudflare fixture ids.
#[derive(Debug, Clone)]
pub struct CfFixture {
    pub client_id: String,
    pub tunnel_id: String,
    pub cf_tunnel_id: String,
    pub zone_id: String,
}

pub const CF_ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
pub const CF_TOKEN: &str = "cf-test-token-value";

impl super::TestCore {
    /// A running Traefik with a static config that satisfies the prerequisite checklist.
    pub fn enable_traefik(&self) {
        let t = &self.config.ingress.traefik;
        self.ingress.systemd.install(
            "traefik",
            true,
            "/usr/local/bin/traefik --configFile=/x/traefik.yml",
        );
        self.ingress
            .systemd
            .set_version("/usr/local/bin/traefik", "Version: 3.fake");
        self.ingress.traefik.set_static(
            &t.static_config,
            &format!(
                "entryPoints:\n  websecure:\n    address: ':443'\nproviders:\n  file:\n    directory: {}\n    watch: true\ncertificatesResolvers:\n  {}:\n    acme:\n      email: a@b.c\n",
                t.dynamic_dir.display(),
                t.cert_resolver
            ),
        );
    }

    /// Declare a loopback port on a Project (creating the Project row when needed).
    pub async fn ensure_port(&self, project: &str, host_port: u16) -> String {
        self.ensure_project(project).await;
        let id = format!("ppt_{project}_{host_port}");
        sqlx::query(
            "INSERT OR IGNORE INTO project_ports (id, project_id, label, host_port, container_port, created_at)
             VALUES (?, ?, ?, ?, 80, 0)",
        )
        .bind(&id)
        .bind(project)
        .bind(format!("p{host_port}"))
        .bind(i64::from(host_port))
        .execute(self.db.writer())
        .await
        .expect("insert port");
        id
    }

    /// A Cloudflare client, a zone `example.com` and an active tunnel, with `cloudflared` running.
    pub async fn setup_cloudflare(&self) -> CfFixture {
        use crate::actor::Actor;
        self.ingress.cloudflare.accept_token(CF_TOKEN);
        self.ingress.cloudflare.add_zone("zone-1", "example.com");
        self.ingress
            .systemd
            .install("cloudflared", true, "/usr/bin/cloudflared tunnel run");
        let cli = Actor::cli();
        let client = self
            .services
            .cloudflare
            .client_create(&cli, "main", CF_ACCOUNT, &Secret::new(CF_TOKEN))
            .await
            .expect("client");
        let t = self
            .services
            .cloudflare
            .tunnel_create(&cli, &client.id, "vps")
            .await
            .expect("tunnel")
            .tunnel;
        CfFixture {
            client_id: client.id,
            tunnel_id: t.id,
            cf_tunnel_id: t.cf_tunnel_id,
            zone_id: "zone-1".into(),
        }
    }
}
