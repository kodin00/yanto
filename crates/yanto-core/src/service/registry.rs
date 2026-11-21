//! Hosted Registry (docs/rewrite/04 section 4, D10, ADR 0006): a Yanto-managed `registry:2`
//! container on `127.0.0.1:<port>` with htpasswd users, a repo/tag browser, tag delete and
//! garbage collection. Everything here is Owner-only.
//!
//! The container is driven through the shared `docker` CLI port (as M2 does); the registry
//! HTTP API goes through `RegistryApiPort`. One mutex serializes enable/disable, user
//! changes, restarts and garbage collection (04 4.3).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use crate::actor::{Access, Actor, Channel, OpSpec, Origin, Scope};
use crate::crypto::new_id;
use crate::db::repo::db_err;
use crate::db::repo::registry::{self as repo, RegistryUserPublic, SettingsRow, UserRow};
use crate::deploy::files;
use crate::error::{CoreError, ErrorKind};
use crate::hosted_registry as hr;
use crate::ports::{
    CancellationToken, CommandLogStream, CommandPort, CommandSpec, DockerPort, HostProbePort,
    LogSink, ManifestInfo, RegistryApiError, RegistryApiPort, RegistryDeps, RegistryEndpoint,
};
use crate::secrets::Secret;

pub const OP_REGISTRY_STATUS: OpSpec = OpSpec::new("registry_status", Access::Owner, Scope::Read);
pub const OP_REGISTRY_ENABLE: OpSpec = OpSpec::new("registry_enable", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_DISABLE: OpSpec =
    OpSpec::new("registry_disable", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_SETTINGS_UPDATE: OpSpec =
    OpSpec::new("registry_settings_update", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_RESTART: OpSpec =
    OpSpec::new("registry_restart", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_RERENDER: OpSpec =
    OpSpec::new("registry_rerender", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_USERS_LIST: OpSpec =
    OpSpec::new("registry_users_list", Access::Owner, Scope::Read);
pub const OP_REGISTRY_USER_CREATE: OpSpec =
    OpSpec::new("registry_user_create", Access::Owner, Scope::Write);
pub const OP_REGISTRY_USER_PASSWORD_SET: OpSpec =
    OpSpec::new("registry_user_password_set", Access::Owner, Scope::Write);
pub const OP_REGISTRY_USER_DELETE: OpSpec =
    OpSpec::new("registry_user_delete", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_REPOS_LIST: OpSpec =
    OpSpec::new("registry_repos_list", Access::Owner, Scope::Read);
pub const OP_REGISTRY_TAGS_LIST: OpSpec =
    OpSpec::new("registry_tags_list", Access::Owner, Scope::Read);
pub const OP_REGISTRY_TAG_DELETE_PREVIEW: OpSpec =
    OpSpec::new("registry_tag_delete_preview", Access::Owner, Scope::Read);
pub const OP_REGISTRY_TAG_DELETE: OpSpec =
    OpSpec::new("registry_tag_delete", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_GC_PREVIEW: OpSpec =
    OpSpec::new("registry_gc_preview", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_GC: OpSpec = OpSpec::new("registry_gc", Access::Owner, Scope::Admin);
pub const OP_REGISTRY_GC_STATUS: OpSpec =
    OpSpec::new("registry_gc_status", Access::Owner, Scope::Read);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_REGISTRY_STATUS,
    &OP_REGISTRY_ENABLE,
    &OP_REGISTRY_DISABLE,
    &OP_REGISTRY_SETTINGS_UPDATE,
    &OP_REGISTRY_RESTART,
    &OP_REGISTRY_RERENDER,
    &OP_REGISTRY_USERS_LIST,
    &OP_REGISTRY_USER_CREATE,
    &OP_REGISTRY_USER_PASSWORD_SET,
    &OP_REGISTRY_USER_DELETE,
    &OP_REGISTRY_REPOS_LIST,
    &OP_REGISTRY_TAGS_LIST,
    &OP_REGISTRY_TAG_DELETE_PREVIEW,
    &OP_REGISTRY_TAG_DELETE,
    &OP_REGISTRY_GC_PREVIEW,
    &OP_REGISTRY_GC,
    &OP_REGISTRY_GC_STATUS,
];

/// Typed confirmation for removing the stored images when disabling.
pub const DELETE_DATA_PHRASE: &str = "delete registry data";

const PAGE_DEFAULT: u32 = 100;
const PAGE_MAX: u32 = 200;
/// Tag listings stop here when resolving sibling tags of a digest.
const SIBLING_SCAN_MAX: usize = 5000;
const GC_OUTPUT_MAX: usize = 256 * 1024;

#[derive(Debug, Clone, Default)]
pub struct EnableInput {
    /// Loopback port to publish; keeps the stored value (default 5000) when `None`.
    pub listen_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerState {
    pub state: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum Health {
    Disabled,
    Healthy,
    /// Not reachable (container missing, stopped or starting).
    Down(String),
    /// 401 with the internal credential: the container holds an older htpasswd file.
    AuthMismatch,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryStatus {
    pub enabled: bool,
    pub listen_port: u16,
    /// `127.0.0.1:<port>`: the host part of image references.
    pub image_host: String,
    pub container_name: String,
    pub container: Option<ContainerState>,
    pub health: Health,
    pub image_digest: Option<String>,
    pub public_host: Option<String>,
    pub users: usize,
    pub disk_bytes: Option<u64>,
    pub gc_in_progress: bool,
    /// The flag is set but no collection is running here: the registry may be left read-only.
    pub gc_left_readonly: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserCreated {
    pub user: RegistryUserPublic,
    /// Present only when Yanto generated the password; shown once, never stored.
    #[serde(skip)]
    pub generated_password: Option<Secret>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoPage {
    pub repos: Vec<String>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagInfo {
    pub tag: String,
    pub digest: String,
    pub media_type: String,
    /// Config plus layer bytes; `None` for indexes and when the manifest could not be read.
    pub size: Option<u64>,
    /// Number of child manifests for a multi-arch index.
    pub children: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagPage {
    pub repo: String,
    pub tags: Vec<TagInfo>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagDeletePreview {
    pub repo: String,
    pub tag: String,
    pub digest: String,
    /// Other tags of the repository that point at the same digest and go away with it.
    pub also_removed: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GcState {
    Idle,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct GcStatus {
    pub state: GcState,
    pub delete_untagged: bool,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub reclaimed_bytes: Option<u64>,
    pub error: Option<String>,
    /// Combined output of the collection, last 256 KiB.
    pub output: String,
}

impl GcStatus {
    fn idle() -> Self {
        Self {
            state: GcState::Idle,
            delete_untagged: false,
            started_at: None,
            finished_at: None,
            reclaimed_bytes: None,
            error: None,
            output: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GcPreview {
    pub delete_untagged: bool,
    pub output: String,
}

struct Inner {
    command: Arc<dyn CommandPort>,
    docker: Arc<dyn DockerPort>,
    api: Arc<dyn RegistryApiPort>,
    probe: Arc<dyn HostProbePort>,
    data_dir: PathBuf,
    server_port: u16,
    bcrypt_cost: u32,
    startup_wait: Duration,
    /// Held by every operation that changes the container or its files.
    lock: Arc<AsyncMutex<()>>,
    gc: Mutex<GcStatus>,
    gc_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct RegistryService {
    ctx: Ctx,
    inner: Arc<Inner>,
}

impl std::fmt::Debug for RegistryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryService").finish_non_exhaustive()
    }
}

struct TextSink(Mutex<String>);

impl LogSink for TextSink {
    fn write(&self, _: CommandLogStream, text: &str) {
        let mut s = self.0.lock().expect("text sink");
        if s.len() < GC_OUTPUT_MAX {
            s.push_str(text);
        }
    }
}

fn api_err(e: RegistryApiError) -> CoreError {
    match e {
        RegistryApiError::Unreachable(_) => CoreError::new(
            ErrorKind::Unavailable,
            "the Hosted Registry is not reachable; check that its container is running",
        ),
        RegistryApiError::Unauthorized => CoreError::new(
            ErrorKind::Unavailable,
            "the Hosted Registry rejected Yanto's internal credential; restart the registry to reload its users",
        ),
        RegistryApiError::NotFound => CoreError::not_found("repository, tag or manifest not found"),
        RegistryApiError::DeleteDisabled => CoreError::conflict(
            "deleting is disabled in the registry configuration; re-render the configuration and restart",
        ),
        RegistryApiError::Other { status, message } => CoreError::new(
            ErrorKind::Unavailable,
            format!("the registry answered {status}: {message}"),
        ),
    }
}

fn dir_size(path: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

fn clamp_page(n: Option<u32>) -> u32 {
    n.unwrap_or(PAGE_DEFAULT).clamp(1, PAGE_MAX)
}

fn system_actor() -> AuditActor {
    AuditActor::system(&Origin::new(Channel::System, None))
}

impl RegistryService {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        ctx: Ctx,
        deps: RegistryDeps,
        command: Arc<dyn CommandPort>,
        docker: Arc<dyn DockerPort>,
        probe: Arc<dyn HostProbePort>,
        data_dir: PathBuf,
        server_port: u16,
    ) -> Self {
        let data_dir = std::path::absolute(&data_dir).unwrap_or(data_dir);
        Self {
            ctx,
            inner: Arc::new(Inner {
                command,
                docker,
                api: deps.api,
                probe,
                data_dir,
                server_port,
                bcrypt_cost: deps.bcrypt_cost,
                startup_wait: deps.startup_wait,
                lock: Arc::new(AsyncMutex::new(())),
                gc: Mutex::new(GcStatus::idle()),
                gc_task: Mutex::new(None),
            }),
        }
    }

    // ---- paths ----------------------------------------------------------------------

    fn root(&self) -> PathBuf {
        self.inner.data_dir.join("registry")
    }
    fn conf_dir(&self) -> PathBuf {
        self.root().join("conf")
    }
    fn auth_dir(&self) -> PathBuf {
        self.root().join("auth")
    }
    fn data_path(&self) -> PathBuf {
        self.root().join("data")
    }

    // ---- shared helpers -------------------------------------------------------------

    async fn settings(&self) -> Result<SettingsRow, CoreError> {
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        repo::settings(&mut conn).await
    }

    fn internal_password(&self, s: &SettingsRow) -> Result<Secret, CoreError> {
        let enc = s
            .internal_password_enc
            .as_deref()
            .ok_or_else(|| CoreError::conflict("the Hosted Registry is not enabled"))?;
        self.ctx.secrets.open(
            "hosted_registry_settings",
            "internal_password_enc",
            "1",
            enc,
        )
    }

    fn endpoint(&self, s: &SettingsRow) -> Result<RegistryEndpoint, CoreError> {
        Ok(RegistryEndpoint {
            port: u16::try_from(s.listen_port).unwrap_or(hr::DEFAULT_PORT),
            username: hr::INTERNAL_USER.to_owned(),
            password: self.internal_password(s)?,
        })
    }

    async fn enabled_endpoint(&self) -> Result<RegistryEndpoint, CoreError> {
        let s = self.settings().await?;
        if !s.enabled {
            return Err(CoreError::conflict("the Hosted Registry is not enabled"));
        }
        self.endpoint(&s)
    }

    async fn docker(
        &self,
        args: Vec<String>,
        timeout: Duration,
        capture: bool,
    ) -> Result<String, CoreError> {
        let mut spec = CommandSpec::args_owned("docker", args).timeout(timeout);
        if capture {
            spec = spec.capture();
        }
        let sink = Arc::new(TextSink(Mutex::new(String::new())));
        let out = self
            .inner
            .command
            .run(spec.clone(), sink.clone(), &CancellationToken::new())
            .await?;
        if out.success() {
            return Ok(out.stdout);
        }
        let tail = sink.0.lock().expect("sink").trim().to_owned();
        let tail: String = tail.lines().rev().take(4).collect::<Vec<_>>().join(" | ");
        Err(CoreError::new(
            ErrorKind::Unavailable,
            format!(
                "`docker {}` failed: {tail}",
                spec.args.first().map(String::as_str).unwrap_or("")
            ),
        ))
    }

    async fn find_container(
        &self,
        name: &str,
    ) -> Result<Option<crate::ports::ContainerInfo>, CoreError> {
        let all = self
            .inner
            .docker
            .list_containers()
            .await
            .map_err(super::containers::map_docker_err)?;
        Ok(all
            .into_iter()
            .find(|c| c.name.trim_start_matches('/') == name))
    }

    /// Write `config.yml` and `htpasswd` from the database state. Called before the container
    /// starts (the registry creates its own htpasswd if the file is missing) and on every change.
    async fn write_files(
        &self,
        conn: &mut sqlx::SqliteConnection,
        s: &SettingsRow,
        readonly: bool,
    ) -> Result<(), CoreError> {
        let internal = self.internal_password(s)?;
        let cost = self.inner.bcrypt_cost;
        let internal_hash =
            tokio::task::spawn_blocking(move || hr::hash_password(internal.expose(), cost))
                .await
                .map_err(|e| CoreError::internal("hashing task failed", e))??;
        let users = repo::users(conn).await?;
        let htpasswd = {
            let mut entries: Vec<(&str, &str)> = vec![(hr::INTERNAL_USER, internal_hash.as_str())];
            entries.extend(
                users
                    .iter()
                    .map(|u| (u.username.as_str(), u.password_hash.as_str())),
            );
            hr::render_htpasswd(entries)
        };
        let conf = self.conf_dir();
        let auth = self.auth_dir();
        let data = self.data_path();
        let config = hr::render_config(readonly);
        tokio::task::spawn_blocking(move || -> Result<(), CoreError> {
            files::ensure_dir(&data, 0o755)?;
            files::ensure_dir(&conf, 0o755)?;
            files::ensure_dir(&auth, 0o755)?;
            files::write_atomic(&conf.join("config.yml"), &config, 0o644)?;
            files::write_atomic(&auth.join("htpasswd"), &htpasswd, 0o600)
        })
        .await
        .map_err(|e| CoreError::internal("file task failed", e))?
    }

    async fn wait_ready(&self, s: &SettingsRow) -> Result<(), CoreError> {
        let ep = self.endpoint(s)?;
        let deadline = tokio::time::Instant::now() + self.inner.startup_wait;
        let step = (self.inner.startup_wait / 20)
            .clamp(Duration::from_millis(5), Duration::from_millis(500));
        loop {
            match self.inner.api.ping(&ep).await {
                Ok(()) => return Ok(()),
                Err(RegistryApiError::Unauthorized) => {
                    return Err(api_err(RegistryApiError::Unauthorized));
                }
                Err(e) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(self.start_failure(s, &e).await);
                    }
                }
            }
            tokio::time::sleep(step).await;
        }
    }

    /// Explain why the registry does not answer: container state and its last log lines.
    async fn start_failure(&self, s: &SettingsRow, e: &RegistryApiError) -> CoreError {
        let mut msg = format!("the registry did not become ready ({e})");
        if let Ok(Some(c)) = self.find_container(&s.container_name).await {
            msg.push_str(&format!("; container is {}", c.status));
            if let Ok(lines) = self.inner.docker.logs(&c.id, 8).await {
                let tail: Vec<String> = lines.into_iter().map(|l| l.text).collect();
                if !tail.is_empty() {
                    msg.push_str(&format!("; last log lines: {}", tail.join(" | ")));
                }
            }
        } else {
            msg.push_str("; the container does not exist");
        }
        CoreError::new(ErrorKind::Unavailable, msg)
    }

    async fn restart_container(&self, s: &SettingsRow) -> Result<(), CoreError> {
        self.docker(
            vec![
                "restart".into(),
                "-t".into(),
                "5".into(),
                s.container_name.clone(),
            ],
            Duration::from_secs(60),
            false,
        )
        .await?;
        self.wait_ready(s).await
    }

    /// After a user change: the registry loads htpasswd once, so restart it (04 4.2).
    async fn restart_after_change(&self, s: &SettingsRow) -> Result<(), CoreError> {
        if !s.enabled {
            return Ok(());
        }
        self.restart_container(s).await.map_err(|e| {
            CoreError::new(
                ErrorKind::Unavailable,
                format!("the change is saved, but restarting the registry failed: {e}"),
            )
        })
    }

    // ---- status ---------------------------------------------------------------------

    pub async fn status(&self, actor: &Actor) -> Result<RegistryStatus, CoreError> {
        actor.authorize(&OP_REGISTRY_STATUS, None)?;
        let s = self.settings().await?;
        let users = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            repo::users(&mut conn).await?.len()
        };
        let port = u16::try_from(s.listen_port).unwrap_or(hr::DEFAULT_PORT);
        let container = self
            .find_container(&s.container_name)
            .await
            .ok()
            .flatten()
            .map(|c| ContainerState {
                state: c.state,
                status: c.status,
            });
        let health = if !s.enabled {
            Health::Disabled
        } else {
            match self.endpoint(&s) {
                Err(_) => Health::Down("the internal credential is missing".into()),
                Ok(ep) => match self.inner.api.ping(&ep).await {
                    Ok(()) => Health::Healthy,
                    Err(RegistryApiError::Unauthorized) => Health::AuthMismatch,
                    Err(e) => Health::Down(e.to_string()),
                },
            }
        };
        let disk_bytes = if s.enabled {
            let p = self.data_path();
            tokio::task::spawn_blocking(move || dir_size(&p)).await.ok()
        } else {
            None
        };
        let running = self.inner.gc.lock().expect("gc").state == GcState::Running;
        Ok(RegistryStatus {
            enabled: s.enabled,
            listen_port: port,
            image_host: hr::local_host(port),
            container_name: s.container_name,
            container,
            health,
            image_digest: s.image_digest,
            public_host: s.public_host,
            users,
            disk_bytes,
            gc_in_progress: s.gc_in_progress && running,
            gc_left_readonly: s.gc_in_progress && !running,
        })
    }

    // ---- enable / disable -----------------------------------------------------------

    async fn check_port(&self, port: u16) -> Result<(), CoreError> {
        if port < 1024 {
            return Err(CoreError::validation(
                "listen_port",
                "the port must be between 1024 and 65535",
            ));
        }
        if port == self.inner.server_port {
            return Err(CoreError::validation(
                "listen_port",
                "this is the port Yanto itself listens on",
            ));
        }
        let used: Option<i64> = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            sqlx::query_scalar("SELECT 1 FROM project_ports WHERE host_port = ? LIMIT 1")
                .bind(i64::from(port))
                .fetch_optional(&mut *conn)
                .await
                .map_err(db_err)?
        };
        if used.is_some() {
            return Err(CoreError::conflict(format!(
                "port {port} is already declared by a Project"
            )));
        }
        if let Some(l) = self
            .inner
            .probe
            .listeners()
            .await?
            .into_iter()
            .find(|l| l.port == port && l.protocol == "tcp")
        {
            return Err(CoreError::conflict(format!(
                "port {port} is already in use on the Host{}",
                l.process.map(|p| format!(" by {p}")).unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// Pull `registry:2` once and record the digest it resolved to (OQ-21).
    async fn pin_image(&self) -> Result<String, CoreError> {
        self.docker(
            vec!["pull".into(), hr::IMAGE_TAG.into()],
            Duration::from_secs(900),
            false,
        )
        .await?;
        let out = self
            .docker(
                vec![
                    "image".into(),
                    "inspect".into(),
                    hr::IMAGE_TAG.into(),
                    "--format".into(),
                    "{{json .RepoDigests}}".into(),
                ],
                Duration::from_secs(30),
                true,
            )
            .await?;
        let digests: Vec<String> = serde_json::from_str(out.trim()).unwrap_or_default();
        digests
            .into_iter()
            .find(|d| d.contains("@sha256:"))
            .ok_or_else(|| {
                CoreError::new(
                    ErrorKind::Unavailable,
                    "the pulled registry image has no repo digest to pin",
                )
            })
    }

    pub async fn enable(
        &self,
        actor: &Actor,
        input: &EnableInput,
    ) -> Result<RegistryStatus, CoreError> {
        actor.authorize(&OP_REGISTRY_ENABLE, None)?;
        let _guard = self.inner.lock.lock().await;
        let mut s = self.settings().await?;
        if s.enabled {
            return Err(CoreError::conflict(
                "the Hosted Registry is already enabled",
            ));
        }
        let port = input
            .listen_port
            .unwrap_or_else(|| u16::try_from(s.listen_port).unwrap_or(hr::DEFAULT_PORT));
        self.inner
            .docker
            .ping()
            .await
            .map_err(super::containers::map_docker_err)?;
        // A container of ours left by an earlier enable would hold the port.
        if let Some(c) = self.find_container(&s.container_name).await? {
            if c.labels
                .get(crate::ports::MANAGED_LABEL)
                .map(String::as_str)
                != Some("registry")
            {
                return Err(CoreError::conflict(format!(
                    "a container named {} exists that Yanto does not manage",
                    s.container_name
                )));
            }
            self.docker(
                vec!["rm".into(), "-f".into(), s.container_name.clone()],
                Duration::from_secs(60),
                false,
            )
            .await?;
        }
        self.check_port(port).await?;
        if s.image_digest.is_none() {
            s.image_digest = Some(self.pin_image().await?);
        }
        if s.internal_password_enc.is_none() {
            s.internal_password_enc = Some(self.ctx.secrets.seal(
                "hosted_registry_settings",
                "internal_password_enc",
                "1",
                &Secret::new(hr::generate_password()),
            )?);
        }
        s.listen_port = i64::from(port);
        {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            self.write_files(&mut conn, &s, false).await?;
        }
        let root = self.root();
        let args: Vec<String> = [
            "run".to_owned(),
            "-d".into(),
            "--name".into(),
            s.container_name.clone(),
            "--restart".into(),
            "unless-stopped".into(),
            "--label".into(),
            format!("{}=registry", crate::ports::MANAGED_LABEL),
            "-p".into(),
            format!("127.0.0.1:{port}:5000"),
            "-v".into(),
            format!("{}:/etc/yanto:ro", root.join("conf").display()),
            "-v".into(),
            format!("{}:/auth:ro", root.join("auth").display()),
            "-v".into(),
            format!("{}:/var/lib/registry", root.join("data").display()),
            s.image_digest.clone().unwrap_or_default(),
            "serve".into(),
            hr::CONTAINER_CONFIG.into(),
        ]
        .into();
        self.docker(args, Duration::from_secs(120), false).await?;
        if let Err(e) = self.wait_ready(&s).await {
            let _ = self
                .docker(
                    vec!["rm".into(), "-f".into(), s.container_name.clone()],
                    Duration::from_secs(60),
                    false,
                )
                .await;
            return Err(e);
        }
        s.enabled = true;
        s.updated_at = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        repo::save_settings(&mut tx, &s).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("registry.enable", "registry")
                .meta(json!({ "listen_port": port, "image": s.image_digest })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        drop(_guard);
        self.status(actor).await
    }

    /// Stop and remove the container. Images and files stay unless `delete_data` carries the
    /// typed phrase [`DELETE_DATA_PHRASE`].
    pub async fn disable(
        &self,
        actor: &Actor,
        delete_data: Option<&str>,
    ) -> Result<RegistryStatus, CoreError> {
        actor.authorize(&OP_REGISTRY_DISABLE, None)?;
        let wipe = match delete_data.map(str::trim).filter(|v| !v.is_empty()) {
            None => false,
            Some(p) if p == DELETE_DATA_PHRASE => true,
            Some(_) => {
                return Err(CoreError::validation(
                    "confirm",
                    format!("type \"{DELETE_DATA_PHRASE}\" to delete the stored images"),
                ));
            }
        };
        let _guard = self
            .inner
            .lock
            .try_lock()
            .map_err(|_| CoreError::conflict("another registry operation is running"))?;
        let mut s = self.settings().await?;
        if !s.enabled {
            return Err(CoreError::conflict("the Hosted Registry is not enabled"));
        }
        if let Err(e) = self
            .docker(
                vec!["rm".into(), "-f".into(), s.container_name.clone()],
                Duration::from_secs(60),
                false,
            )
            .await
        {
            // Already gone is fine; anything else keeps the registry marked enabled.
            if self.find_container(&s.container_name).await?.is_some() {
                return Err(e);
            }
        }
        if wipe {
            let data = self.data_path();
            tokio::task::spawn_blocking(move || {
                if data.exists() {
                    std::fs::remove_dir_all(&data)
                } else {
                    Ok(())
                }
            })
            .await
            .map_err(|e| CoreError::internal("delete task failed", e))?
            .map_err(|e| CoreError::internal("cannot delete registry data", e))?;
        }
        s.enabled = false;
        s.gc_in_progress = false;
        s.updated_at = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        repo::save_settings(&mut tx, &s).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("registry.disable", "registry").meta(json!({ "delete_data": wipe })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        drop(_guard);
        self.status(actor).await
    }

    /// `public_host` is display only (the Hostname that fronts the registry); empty clears it.
    pub async fn settings_update(
        &self,
        actor: &Actor,
        public_host: Option<&str>,
    ) -> Result<RegistryStatus, CoreError> {
        actor.authorize(&OP_REGISTRY_SETTINGS_UPDATE, None)?;
        let host = match public_host.map(|h| h.trim().to_ascii_lowercase()) {
            None => None,
            Some(h) if h.is_empty() => None,
            Some(h) => {
                let h = h
                    .trim_start_matches("https://")
                    .trim_end_matches('/')
                    .to_owned();
                let ok = h.len() <= 255
                    && !h.starts_with('-')
                    && h.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
                if !ok {
                    return Err(CoreError::validation(
                        "public_host",
                        "public host must look like registry.example.com",
                    ));
                }
                Some(h)
            }
        };
        let mut tx = self.ctx.db.begin_write().await?;
        let mut s = repo::settings(&mut tx).await?;
        s.public_host = host.clone();
        s.updated_at = self.ctx.now();
        repo::save_settings(&mut tx, &s).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("registry.settings_update", "registry")
                .meta(json!({ "public_host": host })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        self.status(actor).await
    }

    /// Restart the container (reloads htpasswd; the fix for the "auth mismatch" banner).
    pub async fn restart(&self, actor: &Actor) -> Result<RegistryStatus, CoreError> {
        actor.authorize(&OP_REGISTRY_RESTART, None)?;
        let _guard = self
            .inner
            .lock
            .try_lock()
            .map_err(|_| CoreError::conflict("another registry operation is running"))?;
        let s = self.settings().await?;
        if !s.enabled {
            return Err(CoreError::conflict("the Hosted Registry is not enabled"));
        }
        self.restart_container(&s).await?;
        self.audit_standalone(actor, "registry.restart", json!({}))
            .await?;
        drop(_guard);
        self.status(actor).await
    }

    /// Rewrite `config.yml` and `htpasswd` from the database and restart (04 4.8 recovery action).
    pub async fn rerender(&self, actor: &Actor) -> Result<RegistryStatus, CoreError> {
        actor.authorize(&OP_REGISTRY_RERENDER, None)?;
        let _guard = self
            .inner
            .lock
            .try_lock()
            .map_err(|_| CoreError::conflict("another registry operation is running"))?;
        let s = self.settings().await?;
        if !s.enabled {
            return Err(CoreError::conflict("the Hosted Registry is not enabled"));
        }
        {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            self.write_files(&mut conn, &s, false).await?;
        }
        self.restart_container(&s).await?;
        self.audit_standalone(actor, "registry.rerender", json!({}))
            .await?;
        drop(_guard);
        self.status(actor).await
    }

    async fn audit_standalone(
        &self,
        actor: &Actor,
        action: &'static str,
        meta: serde_json::Value,
    ) -> Result<(), CoreError> {
        audit::write_standalone(
            &self.ctx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new(action, "registry").meta(meta),
        )
        .await
    }

    // ---- users ----------------------------------------------------------------------

    pub async fn users_list(&self, actor: &Actor) -> Result<Vec<RegistryUserPublic>, CoreError> {
        actor.authorize(&OP_REGISTRY_USERS_LIST, None)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        Ok(repo::users(&mut conn)
            .await?
            .iter()
            .map(RegistryUserPublic::from)
            .collect())
    }

    async fn hash(&self, password: &str) -> Result<String, CoreError> {
        let (pw, cost) = (password.to_owned(), self.inner.bcrypt_cost);
        tokio::task::spawn_blocking(move || hr::hash_password(&pw, cost))
            .await
            .map_err(|e| CoreError::internal("hashing task failed", e))?
    }

    /// Create a user. `password = None` generates one, returned once in
    /// [`UserCreated::generated_password`].
    pub async fn user_create(
        &self,
        actor: &Actor,
        username: &str,
        password: Option<&Secret>,
    ) -> Result<UserCreated, CoreError> {
        actor.authorize(&OP_REGISTRY_USER_CREATE, None)?;
        let name = hr::validate_username(username)?;
        let (pw, generated) = match password {
            Some(p) => {
                hr::validate_password(p.expose())?;
                (p.clone(), false)
            }
            None => (Secret::new(hr::generate_password()), true),
        };
        let hash = self.hash(pw.expose()).await?;
        let _guard = self.inner.lock.lock().await;
        let now = self.ctx.now();
        let row = UserRow {
            id: new_id("rus"),
            username: name,
            password_hash: hash,
            created_at: now,
            updated_at: now,
        };
        let mut tx = self.ctx.db.begin_write().await?;
        repo::insert_user(&mut tx, &row).await?;
        let s = repo::settings(&mut tx).await?;
        if s.enabled {
            // Written before the commit: a failed write rolls the user back.
            self.write_files(&mut tx, &s, false).await?;
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("registry_user.create", "registry_user")
                .entity(&row.id)
                .meta(json!({ "username": row.username, "generated_password": generated })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        self.restart_after_change(&s).await?;
        Ok(UserCreated {
            user: RegistryUserPublic::from(&row),
            generated_password: generated.then_some(pw),
        })
    }

    /// Set a password. `None` generates one, returned once.
    pub async fn user_password_set(
        &self,
        actor: &Actor,
        id: &str,
        password: Option<&Secret>,
    ) -> Result<Option<Secret>, CoreError> {
        actor.authorize(&OP_REGISTRY_USER_PASSWORD_SET, None)?;
        let (pw, generated) = match password {
            Some(p) => {
                hr::validate_password(p.expose())?;
                (p.clone(), false)
            }
            None => (Secret::new(hr::generate_password()), true),
        };
        let hash = self.hash(pw.expose()).await?;
        let _guard = self.inner.lock.lock().await;
        let mut tx = self.ctx.db.begin_write().await?;
        let user = repo::user(&mut tx, id)
            .await?
            .ok_or_else(|| CoreError::not_found("registry user not found"))?;
        repo::set_user_hash(&mut tx, id, &hash, self.ctx.now()).await?;
        let s = repo::settings(&mut tx).await?;
        if s.enabled {
            self.write_files(&mut tx, &s, false).await?;
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("registry_user.password_set", "registry_user")
                .entity(id)
                .meta(json!({ "username": user.username, "generated_password": generated })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        self.restart_after_change(&s).await?;
        Ok(generated.then_some(pw))
    }

    pub async fn user_delete(&self, actor: &Actor, id: &str) -> Result<(), CoreError> {
        actor.authorize(&OP_REGISTRY_USER_DELETE, None)?;
        let _guard = self.inner.lock.lock().await;
        let mut tx = self.ctx.db.begin_write().await?;
        let user = repo::user(&mut tx, id)
            .await?
            .ok_or_else(|| CoreError::not_found("registry user not found"))?;
        repo::delete_user(&mut tx, id).await?;
        let s = repo::settings(&mut tx).await?;
        if s.enabled {
            self.write_files(&mut tx, &s, false).await?;
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("registry_user.delete", "registry_user")
                .entity(id)
                .meta(json!({ "username": user.username })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        self.restart_after_change(&s).await
    }

    // ---- browse ---------------------------------------------------------------------

    pub async fn repos_list(
        &self,
        actor: &Actor,
        last: Option<&str>,
        n: Option<u32>,
    ) -> Result<RepoPage, CoreError> {
        actor.authorize(&OP_REGISTRY_REPOS_LIST, None)?;
        let ep = self.enabled_endpoint().await?;
        let page = self
            .inner
            .api
            .catalog(&ep, last, clamp_page(n))
            .await
            .map_err(api_err)?;
        Ok(RepoPage {
            repos: page.items,
            next: page.next,
        })
    }

    pub async fn tags_list(
        &self,
        actor: &Actor,
        repo_name: &str,
        last: Option<&str>,
        n: Option<u32>,
    ) -> Result<TagPage, CoreError> {
        actor.authorize(&OP_REGISTRY_TAGS_LIST, None)?;
        hr::validate_repo(repo_name)?;
        let ep = self.enabled_endpoint().await?;
        let page = self
            .inner
            .api
            .tags(&ep, repo_name, last, clamp_page(n))
            .await
            .map_err(api_err)?;
        let api = &self.inner.api;
        let infos = futures_util::future::join_all(page.items.iter().map(|tag| {
            let ep = &ep;
            async move {
                match api.manifest(ep, repo_name, tag, false).await {
                    Ok(m) => Ok((tag.clone(), m)),
                    Err(e) => Err((tag.clone(), e)),
                }
            }
        }))
        .await;
        let mut tags = Vec::new();
        for r in infos {
            match r {
                Ok((tag, m)) => tags.push(tag_info(tag, &m)),
                // A tag deleted between the listing and the read simply disappears.
                Err((_, RegistryApiError::NotFound)) => {}
                Err((_, e)) => return Err(api_err(e)),
            }
        }
        Ok(TagPage {
            repo: repo_name.to_owned(),
            tags,
            next: page.next,
        })
    }

    // ---- delete tag -----------------------------------------------------------------

    async fn delete_preview(
        &self,
        ep: &RegistryEndpoint,
        repo_name: &str,
        tag: &str,
    ) -> Result<TagDeletePreview, CoreError> {
        hr::validate_repo(repo_name)?;
        hr::validate_tag(tag)?;
        let api = &self.inner.api;
        let digest = api
            .manifest(ep, repo_name, tag, true)
            .await
            .map_err(api_err)?
            .digest;
        hr::validate_digest(&digest).map_err(|_| {
            CoreError::new(ErrorKind::Unavailable, "the registry returned no digest")
        })?;
        let mut also = Vec::new();
        let mut last: Option<String> = None;
        let mut scanned = 0usize;
        loop {
            let page = api
                .tags(ep, repo_name, last.as_deref(), PAGE_MAX)
                .await
                .map_err(api_err)?;
            for other in page.items.iter().filter(|t| t.as_str() != tag) {
                scanned += 1;
                if let Ok(m) = api.manifest(ep, repo_name, other, true).await
                    && m.digest == digest
                {
                    also.push(other.clone());
                }
            }
            match page.next {
                Some(n) if scanned < SIBLING_SCAN_MAX => last = Some(n),
                _ => break,
            }
        }
        Ok(TagDeletePreview {
            repo: repo_name.to_owned(),
            tag: tag.to_owned(),
            digest,
            also_removed: also,
        })
    }

    pub async fn tag_delete_preview(
        &self,
        actor: &Actor,
        repo_name: &str,
        tag: &str,
    ) -> Result<TagDeletePreview, CoreError> {
        actor.authorize(&OP_REGISTRY_TAG_DELETE_PREVIEW, None)?;
        hr::validate_repo(repo_name)?;
        hr::validate_tag(tag)?;
        let ep = self.enabled_endpoint().await?;
        self.delete_preview(&ep, repo_name, tag).await
    }

    /// Delete the manifest the tag points at (by digest) and with it every sibling tag.
    /// Blobs stay on disk until garbage collection.
    pub async fn tag_delete(
        &self,
        actor: &Actor,
        repo_name: &str,
        tag: &str,
    ) -> Result<TagDeletePreview, CoreError> {
        actor.authorize(&OP_REGISTRY_TAG_DELETE, None)?;
        hr::validate_repo(repo_name)?;
        hr::validate_tag(tag)?;
        let ep = self.enabled_endpoint().await?;
        let _guard = self
            .inner
            .lock
            .try_lock()
            .map_err(|_| CoreError::conflict("another registry operation is running"))?;
        let preview = self.delete_preview(&ep, repo_name, tag).await?;
        self.inner
            .api
            .delete_manifest(&ep, repo_name, &preview.digest)
            .await
            .map_err(api_err)?;
        self.audit_standalone(
            actor,
            "registry.tag_delete",
            json!({
                "repo": repo_name, "tag": tag, "digest": preview.digest,
                "also_removed": preview.also_removed,
            }),
        )
        .await?;
        Ok(preview)
    }

    // ---- garbage collection ---------------------------------------------------------

    fn gc_args(&self, s: &SettingsRow, dry_run: bool, delete_untagged: bool) -> Vec<String> {
        let mut a = vec![
            "exec".to_owned(),
            s.container_name.clone(),
            "registry".into(),
            "garbage-collect".into(),
        ];
        if dry_run {
            a.push("--dry-run".into());
        }
        if delete_untagged {
            a.push("--delete-untagged".into());
        }
        a.push(hr::CONTAINER_CONFIG.into());
        a
    }

    /// `registry garbage-collect --dry-run`: what would be deleted. Does not change the registry.
    pub async fn gc_preview(
        &self,
        actor: &Actor,
        delete_untagged: bool,
    ) -> Result<GcPreview, CoreError> {
        actor.authorize(&OP_REGISTRY_GC_PREVIEW, None)?;
        let s = self.settings().await?;
        if !s.enabled {
            return Err(CoreError::conflict("the Hosted Registry is not enabled"));
        }
        let _guard = self
            .inner
            .lock
            .try_lock()
            .map_err(|_| CoreError::conflict("another registry operation is running"))?;
        let sink = Arc::new(TextSink(Mutex::new(String::new())));
        let out = self
            .inner
            .command
            .run(
                CommandSpec::args_owned("docker", self.gc_args(&s, true, delete_untagged))
                    .timeout(Duration::from_secs(900)),
                sink.clone(),
                &CancellationToken::new(),
            )
            .await?;
        let output = sink.0.lock().expect("sink").clone();
        if !out.success() {
            return Err(CoreError::new(
                ErrorKind::Unavailable,
                format!("garbage-collect dry run failed: {}", output.trim()),
            ));
        }
        Ok(GcPreview {
            delete_untagged,
            output,
        })
    }

    pub async fn gc_status(&self, actor: &Actor) -> Result<GcStatus, CoreError> {
        actor.authorize(&OP_REGISTRY_GC_STATUS, None)?;
        Ok(self.inner.gc.lock().expect("gc").clone())
    }

    /// Start garbage collection in the background and return at once; poll [`Self::gc_status`].
    /// The registry is restarted read-only, collected, and restored to normal mode even when a
    /// step fails (04 4.3).
    pub async fn gc_start(
        &self,
        actor: &Actor,
        delete_untagged: bool,
    ) -> Result<GcStatus, CoreError> {
        actor.authorize(&OP_REGISTRY_GC, None)?;
        let s = self.settings().await?;
        if !s.enabled {
            return Err(CoreError::conflict("the Hosted Registry is not enabled"));
        }
        let guard = self
            .inner
            .lock
            .clone()
            .try_lock_owned()
            .map_err(|_| CoreError::conflict("another registry operation is running"))?;
        let started = self.ctx.now();
        *self.inner.gc.lock().expect("gc") = GcStatus {
            state: GcState::Running,
            delete_untagged,
            started_at: Some(started),
            ..GcStatus::idle()
        };
        {
            let mut tx = self.ctx.db.begin_write().await?;
            let mut s = repo::settings(&mut tx).await?;
            s.gc_in_progress = true;
            s.updated_at = started;
            repo::save_settings(&mut tx, &s).await?;
            tx.commit().await.map_err(db_err)?;
        }
        let me = self.clone();
        let who = AuditActor::from_actor(actor);
        let handle = tokio::spawn(async move {
            let (result, restored) = me.gc_run(delete_untagged).await;
            me.gc_finish(&who, delete_untagged, result, restored, guard)
                .await;
        });
        *self.inner.gc_task.lock().expect("task") = Some(handle);
        Ok(self.inner.gc.lock().expect("gc").clone())
    }

    /// Wait for a running collection to finish (used by tests and graceful shutdown).
    pub async fn gc_wait(&self) {
        let h = self.inner.gc_task.lock().expect("task").take();
        if let Some(h) = h {
            let _ = h.await;
        }
    }

    /// Returns the collection result and whether normal (writable) mode was restored.
    async fn gc_run(&self, delete_untagged: bool) -> (Result<(), CoreError>, bool) {
        let s = match self.settings().await {
            Ok(s) => s,
            Err(e) => return (Err(e), false),
        };
        let data = self.data_path();
        let before = {
            let d = data.clone();
            tokio::task::spawn_blocking(move || dir_size(&d))
                .await
                .unwrap_or(0)
        };
        let step = async {
            {
                let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
                self.write_files(&mut conn, &s, true).await?;
            }
            self.restart_container(&s).await?;
            let out = self
                .inner
                .command
                .run(
                    CommandSpec::args_owned("docker", self.gc_args(&s, false, delete_untagged))
                        .timeout(Duration::from_secs(3600)),
                    Arc::new(GcLive(self.clone())),
                    &CancellationToken::new(),
                )
                .await?;
            if out.success() {
                Ok(())
            } else {
                Err(CoreError::new(
                    ErrorKind::Unavailable,
                    format!("garbage-collect exited with status {}", out.exit_code),
                ))
            }
        }
        .await;
        // Always restore normal mode, whatever happened above.
        let restore = self.restore_normal_mode(&s).await;
        let after = tokio::task::spawn_blocking(move || dir_size(&data))
            .await
            .unwrap_or(0);
        self.inner.gc.lock().expect("gc").reclaimed_bytes = Some(before.saturating_sub(after));
        let restored = restore.is_ok();
        (step.and(restore), restored)
    }

    async fn restore_normal_mode(&self, s: &SettingsRow) -> Result<(), CoreError> {
        {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            self.write_files(&mut conn, s, false).await?;
        }
        self.restart_container(s).await
    }

    async fn gc_finish(
        &self,
        who: &AuditActor,
        delete_untagged: bool,
        result: Result<(), CoreError>,
        restored: bool,
        guard: tokio::sync::OwnedMutexGuard<()>,
    ) {
        let now = self.ctx.now();
        let (outcome_ok, reclaimed) = {
            let mut g = self.inner.gc.lock().expect("gc");
            g.finished_at = Some(now);
            match &result {
                Ok(()) => g.state = GcState::Done,
                Err(e) => {
                    g.state = GcState::Failed;
                    g.error = Some(e.message.clone());
                }
            }
            (result.is_ok(), g.reclaimed_bytes)
        };
        // Clear the persisted flag only when normal mode is known to be restored.
        let write = async {
            let mut tx = self.ctx.db.begin_write().await?;
            let mut s = repo::settings(&mut tx).await?;
            if restored {
                s.gc_in_progress = false;
                s.updated_at = now;
                repo::save_settings(&mut tx, &s).await?;
            }
            let mut ev = AuditEvent::new("registry.gc", "registry").meta(json!({
                "delete_untagged": delete_untagged,
                "reclaimed_bytes": reclaimed,
                "error": result.as_ref().err().map(|e| e.message.clone()),
            }));
            if !outcome_ok {
                ev.outcome = audit::Outcome::Error;
            }
            audit::write(&self.ctx, &mut tx, who, &ev).await?;
            tx.commit().await.map_err(db_err)
        };
        if let Err(e) = write.await {
            tracing::error!(error = %e, "recording the registry GC result failed");
        }
        drop(guard);
    }

    /// Run at `yanto serve` start: when a collection was interrupted (the flag is still set),
    /// restore normal (writable) mode (04 4.3, 4.8).
    pub async fn recover(&self) -> Result<bool, CoreError> {
        let s = self.settings().await?;
        if !s.gc_in_progress {
            return Ok(false);
        }
        let _guard = self.inner.lock.lock().await;
        if s.enabled {
            self.restore_normal_mode(&s).await?;
        }
        let mut tx = self.ctx.db.begin_write().await?;
        let mut s = repo::settings(&mut tx).await?;
        s.gc_in_progress = false;
        s.updated_at = self.ctx.now();
        repo::save_settings(&mut tx, &s).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &system_actor(),
            &AuditEvent::new("registry.gc", "registry").meta(json!({ "recovered": true })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(true)
    }
}

/// Streams command output into the shared GC status.
struct GcLive(RegistryService);

impl LogSink for GcLive {
    fn write(&self, _: CommandLogStream, text: &str) {
        let mut g = self.0.inner.gc.lock().expect("gc");
        g.output.push_str(text);
        if g.output.len() > GC_OUTPUT_MAX {
            let cut = g.output.len() - GC_OUTPUT_MAX;
            let cut = (cut..g.output.len())
                .find(|i| g.output.is_char_boundary(*i))
                .unwrap_or(g.output.len());
            g.output.drain(..cut);
        }
    }
}

fn tag_info(tag: String, m: &ManifestInfo) -> TagInfo {
    let v: serde_json::Value = m
        .body
        .as_deref()
        .and_then(|b| serde_json::from_slice(b).ok())
        .unwrap_or(serde_json::Value::Null);
    let sum = |arr: Option<&Vec<serde_json::Value>>| -> u64 {
        arr.map(|a| {
            a.iter()
                .filter_map(|l| l.get("size").and_then(|s| s.as_u64()))
                .sum()
        })
        .unwrap_or(0)
    };
    let (size, children) = if let Some(ms) = v.get("manifests").and_then(|m| m.as_array()) {
        (None, Some(ms.len()))
    } else if v.get("layers").is_some() {
        let cfg = v
            .get("config")
            .and_then(|c| c.get("size"))
            .and_then(|s| s.as_u64())
            .unwrap_or(0);
        (
            Some(cfg + sum(v.get("layers").and_then(|l| l.as_array()))),
            None,
        )
    } else {
        (None, None)
    };
    TagInfo {
        tag,
        digest: m.digest.clone(),
        media_type: m.media_type.clone(),
        size,
        children,
    }
}
