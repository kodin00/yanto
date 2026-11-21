//! Projects: CRUD, compose and `.env` handling, declared loopback ports, Deploy Token,
//! stop/restart (docs/rewrite/04 1.1-1.3, 03 a.2). Deploying lives in [`super::deployments`].

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use super::deploy_keys;
use super::deployments::DeploymentDto;
use crate::actor::{Access, Actor, ActorKind, Need, OpSpec, Permission, Scope};
use crate::crypto::{new_id, random_token_b64, sha256_hex};
use crate::db::repo::deployments::{self as dep_repo};
use crate::db::repo::projects::{self, PortRow, ProjectRow};
use crate::db::repo::{db_err, deploy_keys as key_repo, registry_credentials};
use crate::deploy::compose::{self, DesiredPort};
use crate::deploy::env::{self, EnvFile, VarChange};
use crate::deploy::runner::{OVERRIDE_FILE, reconcile_ports};
use crate::deploy::tail::TailSink;
use crate::deploy::{Engine, files, keys, portcheck, validate};
use crate::error::CoreError;
use crate::ports::{CancellationToken, CommandSpec, LogSink};
use crate::secrets::Secret;

const MAX_TEXT: usize = 1024 * 1024;

macro_rules! project_op {
    ($name:ident, $str:literal, $need:expr, $scope:expr) => {
        pub const $name: OpSpec = OpSpec::new($str, Access::Project($need), $scope);
    };
}

pub const OP_PROJECT_LIST: OpSpec = OpSpec::new("project_list", Access::Authenticated, Scope::Read);
project_op!(OP_PROJECT_GET, "project_get", Need::View, Scope::Read);
pub const OP_PROJECT_CREATE: OpSpec = OpSpec::new("project_create", Access::Owner, Scope::Write);
project_op!(
    OP_PROJECT_UPDATE,
    "project_update",
    Need::Permission(Permission::Config),
    Scope::Write
);
pub const OP_PROJECT_DELETE: OpSpec = OpSpec::new("project_delete", Access::Owner, Scope::Admin);
project_op!(
    OP_PROJECT_COMPOSE_READ,
    "project_compose_read",
    Need::Permission(Permission::Config),
    Scope::Read
);
project_op!(
    OP_PROJECT_ENV_READ,
    "project_env_read",
    Need::Permission(Permission::Secrets),
    Scope::Read
);
project_op!(
    OP_PROJECT_ENV_CONTENT_READ,
    "project_env_content_read",
    Need::Permission(Permission::Secrets),
    Scope::Read
);
pub const OP_PROJECT_ENV_PREVIEW: OpSpec =
    OpSpec::new("project_env_preview", Access::Authenticated, Scope::Read);
project_op!(
    OP_PROJECT_ENV_WRITE,
    "project_env_write",
    Need::Permission(Permission::Secrets),
    Scope::Write
);
project_op!(
    OP_PROJECT_ENV_VARIABLES_WRITE,
    "project_env_variables_write",
    Need::Permission(Permission::Secrets),
    Scope::Write
);
project_op!(
    OP_PROJECT_PORTS_LIST,
    "project_ports_list",
    Need::View,
    Scope::Read
);
project_op!(
    OP_PROJECT_PORTS_SET,
    "project_ports_set",
    Need::Permission(Permission::Config),
    Scope::Write
);
project_op!(
    OP_PROJECT_DEPLOY_TOKEN_REVEAL,
    "project_deploy_token_reveal",
    Need::Permission(Permission::Secrets),
    Scope::Admin
);
project_op!(
    OP_PROJECT_DEPLOY_TOKEN_ROTATE,
    "project_deploy_token_rotate",
    Need::Permission(Permission::Secrets),
    Scope::Admin
);
project_op!(
    OP_PROJECT_STOP,
    "project_stop",
    Need::Permission(Permission::Runtime),
    Scope::Admin
);
project_op!(
    OP_PROJECT_RESTART,
    "project_restart",
    Need::Permission(Permission::Runtime),
    Scope::Admin
);

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_PROJECT_LIST,
    &OP_PROJECT_GET,
    &OP_PROJECT_CREATE,
    &OP_PROJECT_UPDATE,
    &OP_PROJECT_DELETE,
    &OP_PROJECT_COMPOSE_READ,
    &OP_PROJECT_ENV_READ,
    &OP_PROJECT_ENV_CONTENT_READ,
    &OP_PROJECT_ENV_PREVIEW,
    &OP_PROJECT_ENV_WRITE,
    &OP_PROJECT_ENV_VARIABLES_WRITE,
    &OP_PROJECT_PORTS_LIST,
    &OP_PROJECT_PORTS_SET,
    &OP_PROJECT_DEPLOY_TOKEN_REVEAL,
    &OP_PROJECT_DEPLOY_TOKEN_ROTATE,
    &OP_PROJECT_STOP,
    &OP_PROJECT_RESTART,
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PortDto {
    pub id: String,
    pub label: String,
    pub compose_service: Option<String>,
    pub host_port: i64,
    pub container_port: i64,
    pub bind_address: String,
    pub protocol: String,
}

impl From<PortRow> for PortDto {
    fn from(r: PortRow) -> Self {
        Self {
            id: r.id,
            label: r.label,
            compose_service: r.compose_service,
            host_port: r.host_port,
            container_port: r.container_port,
            bind_address: r.bind_address,
            protocol: r.protocol,
        }
    }
}

/// A Project as adapters see it. Never carries the Deploy Token or any secret.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectDto {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub source_kind: String,
    pub git_url: Option<String>,
    pub branch: Option<String>,
    pub image_ref: Option<String>,
    pub compose_path: String,
    pub has_compose_content: bool,
    pub env_file: String,
    pub auto_start: bool,
    pub manual_deploy_enabled: bool,
    pub github_webhook_enabled: bool,
    pub deploy_key_id: Option<String>,
    pub registry_credential_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub ports: Vec<PortDto>,
    pub last_deployment: Option<DeploymentDto>,
}

impl ProjectDto {
    fn new(p: ProjectRow, ports: Vec<PortRow>, last: Option<dep_repo::DeploymentRow>) -> Self {
        Self {
            has_compose_content: p.compose_content.is_some(),
            id: p.id,
            name: p.name,
            slug: p.slug,
            source_kind: p.source_kind,
            git_url: p.git_url,
            branch: p.branch,
            image_ref: p.image_ref,
            compose_path: p.compose_path,
            env_file: p.env_file,
            auto_start: p.auto_start,
            manual_deploy_enabled: p.manual_deploy_enabled,
            github_webhook_enabled: p.github_webhook_enabled,
            deploy_key_id: p.deploy_key_id,
            registry_credential_id: p.registry_credential_id,
            created_at: p.created_at,
            updated_at: p.updated_at,
            ports: ports.into_iter().map(Into::into).collect(),
            last_deployment: last.map(Into::into),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SourceInput {
    Git {
        url: String,
        branch: Option<String>,
    },
    Image {
        image_ref: String,
    },
    /// One free-form box: classified as in the legacy dashboard.
    Auto(String),
}

#[derive(Debug, Clone)]
pub struct ProjectInput {
    pub name: String,
    pub slug: Option<String>,
    pub source: SourceInput,
    pub compose_path: Option<String>,
    pub compose_content: Option<String>,
    pub env_file: Option<String>,
    pub auto_start: Option<bool>,
    pub manual_deploy_enabled: Option<bool>,
    pub github_webhook_enabled: Option<bool>,
    pub deploy_key_id: Option<String>,
    pub generate_deploy_key: bool,
    pub registry_credential_id: Option<String>,
}

#[derive(Debug)]
pub struct ProjectCreated {
    pub project: ProjectDto,
    /// Shown exactly once.
    pub deploy_token: Secret,
    /// Public half of the generated key, when one was requested.
    pub deploy_key: Option<key_repo::DeployKeyPublic>,
}

/// Partial update; `Option<Option<_>>` fields: `Some(None)` clears.
#[derive(Debug, Clone, Default)]
pub struct ProjectPatch {
    pub name: Option<String>,
    pub git_url: Option<String>,
    pub branch: Option<String>,
    pub image_ref: Option<String>,
    pub compose_path: Option<String>,
    pub compose_content: Option<Option<String>>,
    pub env_file: Option<String>,
    pub auto_start: Option<bool>,
    pub manual_deploy_enabled: Option<bool>,
    pub github_webhook_enabled: Option<bool>,
    pub deploy_key_id: Option<Option<String>>,
    pub registry_credential_id: Option<Option<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct DeleteOptions {
    pub stop_containers: bool,
    pub delete_volumes: bool,
    /// Must equal the slug when `delete_volumes` is set.
    pub confirm_slug: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvVariable {
    pub key: String,
    /// Always the mask placeholder.
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvView {
    pub env_file: String,
    pub exists: bool,
    pub variables: Vec<EnvVariable>,
    pub invalid_lines: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComposeView {
    /// `pasted`, `repository`, `generated` or `none`.
    pub source: &'static str,
    pub path: String,
    pub content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PortInput {
    pub label: String,
    pub compose_service: Option<String>,
    pub host_port: u16,
    pub container_port: u16,
    pub bind_address: String,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StackOutput {
    pub ok: bool,
    pub output: String,
}

#[derive(Debug, Clone)]
pub struct ProjectService {
    ctx: Ctx,
    eng: Arc<Engine>,
}

pub(crate) fn not_found() -> CoreError {
    CoreError::not_found("project not found")
}

pub(crate) fn token_pair(
    ctx: &Ctx,
    project_id: &str,
) -> Result<(Secret, String, String), CoreError> {
    let token = format!("ydp_{}", random_token_b64());
    let secret = Secret::new(token.clone());
    let enc = ctx
        .secrets
        .seal("projects", "deploy_token_enc", project_id, &secret)?;
    Ok((secret, sha256_hex(&token), enc))
}

async fn load(conn: &mut sqlx::SqliteConnection, id: &str) -> Result<ProjectRow, CoreError> {
    projects::get(conn, id).await?.ok_or_else(not_found)
}

fn text_limit(field: &str, text: &str) -> Result<(), CoreError> {
    if text.len() > MAX_TEXT {
        return Err(CoreError::validation(field, "must be at most 1 MiB"));
    }
    Ok(())
}

impl ProjectService {
    pub(super) fn new(ctx: Ctx, eng: Arc<Engine>) -> Self {
        Self { ctx, eng }
    }

    async fn dto(
        &self,
        conn: &mut sqlx::SqliteConnection,
        row: ProjectRow,
    ) -> Result<ProjectDto, CoreError> {
        let ports = projects::ports_for(conn, &row.id).await?;
        let last = dep_repo::list(
            conn,
            &dep_repo::DeploymentFilter {
                project_id: Some(row.id.clone()),
                limit: 1,
                ..Default::default()
            },
        )
        .await?
        .into_iter()
        .next();
        Ok(ProjectDto::new(row, ports, last))
    }

    pub async fn list(&self, actor: &Actor) -> Result<Vec<ProjectDto>, CoreError> {
        actor.authorize(&OP_PROJECT_LIST, None)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let visible = actor.visible_projects();
        let rows = projects::list(&mut conn).await?;
        let mut out = Vec::new();
        for r in rows {
            if visible
                .as_ref()
                .is_some_and(|v| !v.contains(&r.id.as_str()))
            {
                continue;
            }
            out.push(self.dto(&mut conn, r).await?);
        }
        Ok(out)
    }

    pub async fn get(&self, actor: &Actor, id: &str) -> Result<ProjectDto, CoreError> {
        actor.authorize(&OP_PROJECT_GET, Some(id))?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let row = load(&mut conn, id).await?;
        self.dto(&mut conn, row).await
    }

    pub async fn create(
        &self,
        actor: &Actor,
        input: &ProjectInput,
    ) -> Result<ProjectCreated, CoreError> {
        actor.authorize(&OP_PROJECT_CREATE, None)?;
        let name = validate::name(&input.name)?;
        let slug = match input
            .slug
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(s) => validate::slug(s)?,
            None => validate::slug(&validate::slug_from_name(&name)).map_err(|_| {
                CoreError::validation("slug", "could not derive a slug from the name; enter one")
            })?,
        };
        let (git_url, branch, image_ref) = match &input.source {
            SourceInput::Git { url, branch } => (
                Some(validate::git_url(url)?),
                Some(validate::git_ref(
                    "branch",
                    branch.as_deref().unwrap_or("master"),
                )?),
                None,
            ),
            SourceInput::Image { image_ref } => (None, None, Some(validate::image_ref(image_ref)?)),
            SourceInput::Auto(text) => match validate::classify_source(text)? {
                validate::Classified::Git(u) => (
                    Some(validate::git_url(&u)?),
                    Some("master".to_owned()),
                    None,
                ),
                validate::Classified::Image(i) => (None, None, Some(validate::image_ref(&i)?)),
            },
        };
        let compose_path = match &input.compose_path {
            Some(p) if !p.trim().is_empty() => validate::compose_path(p)?,
            _ => "docker-compose.yml".to_owned(),
        };
        let env_file = match &input.env_file {
            Some(p) if !p.trim().is_empty() => validate::env_file(p)?,
            _ => ".env".to_owned(),
        };
        let compose_content = clean_compose(input.compose_content.as_deref())?;
        if input.generate_deploy_key && git_url.is_none() {
            return Err(CoreError::validation(
                "generate_deploy_key",
                "deploy keys are for Git projects",
            ));
        }
        let id = new_id("prj");
        let desired = match &compose_content {
            Some(c) => self.derive_ports(&slug, c).await?,
            None => None,
        };
        if let Some(d) = &desired {
            self.ensure_no_conflicts(&id, &slug, d).await?;
        }
        let (token, token_hash, token_enc) = token_pair(&self.ctx, &id)?;
        let now = self.ctx.now();
        let mut row = ProjectRow {
            id: id.clone(),
            name,
            slug: slug.clone(),
            source_kind: if git_url.is_some() { "git" } else { "image" }.into(),
            git_url,
            branch,
            image_ref,
            compose_path,
            compose_content,
            env_file,
            auto_start: input.auto_start.unwrap_or(true),
            manual_deploy_enabled: input.manual_deploy_enabled.unwrap_or(true),
            github_webhook_enabled: input.github_webhook_enabled.unwrap_or(true),
            deploy_token_hash: token_hash,
            deploy_token_enc: token_enc,
            deploy_key_id: input.deploy_key_id.clone().filter(|s| !s.is_empty()),
            registry_credential_id: input
                .registry_credential_id
                .clone()
                .filter(|s| !s.is_empty()),
            created_at: now,
            updated_at: now,
        };
        let mut tx = self.ctx.db.begin_write().await?;
        self.check_refs(&mut tx, &row).await?;
        let mut generated = None;
        if input.generate_deploy_key {
            let mut key_name = slug.clone();
            if key_repo::list(&mut tx)
                .await?
                .iter()
                .any(|k| k.name == key_name)
            {
                key_name = format!("{slug}-{}", &id[4..10]);
            }
            let k = keys::generate_ed25519(&format!("yanto:{key_name}"))?;
            let krow = deploy_keys::key_row(&self.ctx, &key_name, k)?;
            key_repo::insert(&mut tx, &krow).await?;
            row.deploy_key_id = Some(krow.id.clone());
            audit::write(
                &self.ctx,
                &mut tx,
                &AuditActor::from_actor(actor),
                &AuditEvent::new("deploy_key.create", "deploy_key")
                    .entity(&krow.id)
                    .meta(json!({ "name": krow.name, "fingerprint": krow.fingerprint })),
            )
            .await?;
            generated = Some(key_repo::DeployKeyPublic {
                id: krow.id,
                name: krow.name,
                key_type: krow.key_type,
                public_key: krow.public_key,
                fingerprint: krow.fingerprint,
                created_at: krow.created_at,
            });
        }
        projects::insert(&mut tx, &row).await?;
        if let Some(d) = &desired {
            reconcile_ports(&mut tx, &id, d, now).await?;
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("project.create", "project")
                .entity(&id)
                .project(&id)
                .meta(json!({ "slug": slug, "source_kind": row.source_kind })),
        )
        .await?;
        let dto = self.dto(&mut tx, row).await?;
        tx.commit().await.map_err(db_err)?;
        if let Err(err) = files::ensure_project_dir(&self.eng.data_dir, &slug) {
            tracing::warn!(error = %err, "could not create the project directory; the first deployment will");
        }
        Ok(ProjectCreated {
            project: dto,
            deploy_token: token,
            deploy_key: generated,
        })
    }

    async fn check_refs(
        &self,
        conn: &mut sqlx::SqliteConnection,
        row: &ProjectRow,
    ) -> Result<(), CoreError> {
        if let Some(k) = &row.deploy_key_id
            && key_repo::get_public(conn, k).await?.is_none()
        {
            return Err(CoreError::validation(
                "deploy_key_id",
                "deploy key does not exist",
            ));
        }
        if let Some(c) = &row.registry_credential_id
            && registry_credentials::get(conn, c).await?.is_none()
        {
            return Err(CoreError::validation(
                "registry_credential_id",
                "registry credential does not exist",
            ));
        }
        Ok(())
    }

    /// `docker compose config` on `content` in a scratch directory. `Ok(None)` when Docker
    /// cannot answer (not installed, or the file needs variables that are not set yet).
    async fn derive_ports(
        &self,
        slug: &str,
        content: &str,
    ) -> Result<Option<Vec<DesiredPort>>, CoreError> {
        let scratch = self.eng.data_dir.join("tmp").join(new_id("cfg"));
        files::ensure_dir(&self.eng.data_dir.join("tmp"), 0o700)?;
        files::ensure_dir(&scratch, 0o700)?;
        let result = async {
            files::write_atomic(&scratch.join("compose.yml"), content, 0o600)?;
            let tail = Arc::new(TailSink::default());
            let out = self
                .eng
                .host
                .command
                .run(
                    CommandSpec::new(
                        "docker",
                        &[
                            "compose",
                            "-p",
                            slug,
                            "-f",
                            "compose.yml",
                            "config",
                            "--format",
                            "json",
                        ],
                    )
                    .cwd(scratch.clone())
                    .capture()
                    .timeout(std::time::Duration::from_secs(60)),
                    tail as Arc<dyn LogSink>,
                    &CancellationToken::new(),
                )
                .await;
            match out {
                Ok(o) if o.exit_code == 0 => {
                    match serde_json::from_str::<serde_json::Value>(o.stdout.trim()) {
                        Ok(v) => compose::ports_from_config(&v).map(Some),
                        Err(_) => Ok(None),
                    }
                }
                _ => Ok(None),
            }
        }
        .await;
        let _ = std::fs::remove_dir_all(&scratch);
        result
    }

    async fn ensure_no_conflicts(
        &self,
        project_id: &str,
        slug: &str,
        desired: &[DesiredPort],
    ) -> Result<(), CoreError> {
        let pc = portcheck::check(
            &self.eng,
            project_id,
            slug,
            desired,
            &CancellationToken::new(),
        )
        .await?;
        if pc.conflicts.is_empty() {
            Ok(())
        } else {
            Err(CoreError::conflict(format!(
                "port collision: {}",
                portcheck::describe(&pc.conflicts)
            )))
        }
    }

    pub async fn update(
        &self,
        actor: &Actor,
        id: &str,
        patch: &ProjectPatch,
    ) -> Result<ProjectDto, CoreError> {
        actor.authorize(&OP_PROJECT_UPDATE, Some(id))?;
        let mut row = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            load(&mut conn, id).await?
        };
        let mut changed: Vec<&'static str> = Vec::new();
        if let Some(n) = &patch.name {
            row.name = validate::name(n)?;
            changed.push("name");
        }
        if let Some(u) = &patch.git_url {
            if !row.is_git() {
                return Err(CoreError::validation("git_url", "this is an image project"));
            }
            row.git_url = Some(validate::git_url(u)?);
            changed.push("git_url");
        }
        if let Some(b) = &patch.branch {
            if !row.is_git() {
                return Err(CoreError::validation("branch", "this is an image project"));
            }
            row.branch = Some(validate::git_ref("branch", b)?);
            changed.push("branch");
        }
        if let Some(i) = &patch.image_ref {
            if row.is_git() {
                return Err(CoreError::validation("image_ref", "this is a Git project"));
            }
            row.image_ref = Some(validate::image_ref(i)?);
            changed.push("image_ref");
        }
        if let Some(p) = &patch.compose_path {
            row.compose_path = validate::compose_path(p)?;
            changed.push("compose_path");
        }
        if let Some(p) = &patch.env_file {
            row.env_file = validate::env_file(p)?;
            changed.push("env_file");
        }
        let mut desired = None;
        if let Some(c) = &patch.compose_content {
            row.compose_content = clean_compose(c.as_deref())?;
            changed.push("compose_content");
            if let Some(content) = &row.compose_content {
                desired = self.derive_ports(&row.slug, content).await?;
                if let Some(d) = &desired {
                    self.ensure_no_conflicts(&row.id, &row.slug, d).await?;
                }
            }
        }
        for (field, value, slot) in [
            ("auto_start", patch.auto_start, &mut row.auto_start),
            (
                "manual_deploy_enabled",
                patch.manual_deploy_enabled,
                &mut row.manual_deploy_enabled,
            ),
            (
                "github_webhook_enabled",
                patch.github_webhook_enabled,
                &mut row.github_webhook_enabled,
            ),
        ] {
            if let Some(v) = value {
                *slot = v;
                changed.push(field);
            }
        }
        if let Some(k) = &patch.deploy_key_id {
            row.deploy_key_id = k.clone().filter(|s| !s.is_empty());
            changed.push("deploy_key_id");
        }
        if let Some(c) = &patch.registry_credential_id {
            row.registry_credential_id = c.clone().filter(|s| !s.is_empty());
            changed.push("registry_credential_id");
        }
        row.updated_at = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        self.check_refs(&mut tx, &row).await?;
        projects::save(&mut tx, &row).await?;
        if let Some(d) = &desired {
            reconcile_ports(&mut tx, id, d, row.updated_at).await?;
        }
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("project.update", "project")
                .entity(id)
                .project(id)
                .meta(json!({ "changed": changed })),
        )
        .await?;
        let dto = self.dto(&mut tx, row).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(dto)
    }

    /// Delete a Project: refuses while a Deployment runs; optionally brings the stack down
    /// first; removes the directory (containment-checked) and every row.
    pub async fn delete(
        &self,
        actor: &Actor,
        id: &str,
        opts: &DeleteOptions,
    ) -> Result<(), CoreError> {
        actor.authorize(&OP_PROJECT_DELETE, None)?;
        let row = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            let row = load(&mut conn, id).await?;
            if dep_repo::running_for(&mut conn, id).await?.is_some() {
                return Err(CoreError::conflict(
                    "a deployment is running; cancel it before deleting the project",
                ));
            }
            row
        };
        if opts.delete_volumes && opts.confirm_slug.as_deref() != Some(row.slug.as_str()) {
            return Err(CoreError::validation(
                "confirm_slug",
                "type the project slug to confirm deleting its volumes",
            ));
        }
        if opts.stop_containers {
            let mut args = vec![
                "compose",
                "-p",
                row.slug.as_str(),
                "down",
                "--remove-orphans",
            ];
            if opts.delete_volumes {
                args.push("--volumes");
            }
            let tail = Arc::new(TailSink::default());
            let out = self
                .eng
                .host
                .command
                .run(
                    CommandSpec::new("docker", &args).timeout(std::time::Duration::from_secs(300)),
                    tail.clone() as Arc<dyn LogSink>,
                    &CancellationToken::new(),
                )
                .await?;
            if out.exit_code != 0 {
                return Err(CoreError::conflict(format!(
                    "could not stop the containers (docker compose down exit {}): {}",
                    out.exit_code,
                    tail.text()
                )));
            }
        }
        let mut tx = self.ctx.db.begin_write().await?;
        if dep_repo::running_for(&mut tx, id).await?.is_some() {
            return Err(CoreError::conflict("a deployment started; try again"));
        }
        projects::delete(&mut tx, id).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("project.delete", "project")
                .entity(id)
                .meta(json!({
                    "slug": row.slug,
                    "stopped_containers": opts.stop_containers,
                    "deleted_volumes": opts.delete_volumes,
                })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        if let Err(err) = files::remove_project_dir(&self.eng.data_dir, &row.slug) {
            tracing::warn!(error = %err, slug = %row.slug, "project rows deleted but the directory could not be removed");
        }
        Ok(())
    }

    // ---- compose ------------------------------------------------------------------

    pub async fn compose_read(&self, actor: &Actor, id: &str) -> Result<ComposeView, CoreError> {
        actor.authorize(&OP_PROJECT_COMPOSE_READ, Some(id))?;
        let row = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            load(&mut conn, id).await?
        };
        if let Some(c) = &row.compose_content {
            return Ok(ComposeView {
                source: "pasted",
                path: row.compose_path,
                content: Some(c.clone()),
            });
        }
        let dir = files::project_dir(&self.eng.data_dir, &row.slug)?;
        let on_disk = files::read_optional(&files::resolve_in(&dir, &row.compose_path)?)?;
        let source = match (&on_disk, row.is_git()) {
            (None, _) => "none",
            (Some(_), true) => "repository",
            (Some(_), false) => "generated",
        };
        Ok(ComposeView {
            source,
            path: row.compose_path,
            content: on_disk,
        })
    }

    // ---- env ----------------------------------------------------------------------

    fn env_path(&self, row: &ProjectRow) -> Result<std::path::PathBuf, CoreError> {
        let dir = files::project_dir(&self.eng.data_dir, &row.slug)?;
        files::resolve_in(&dir, &row.env_file)
    }

    async fn env_file_of(&self, id: &str) -> Result<(ProjectRow, Option<String>), CoreError> {
        let row = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            load(&mut conn, id).await?
        };
        let text = files::read_optional(&self.env_path(&row)?)?;
        Ok((row, text))
    }

    /// Variable names with masked values.
    pub async fn env_read(&self, actor: &Actor, id: &str) -> Result<EnvView, CoreError> {
        actor.authorize(&OP_PROJECT_ENV_READ, Some(id))?;
        let (row, text) = self.env_file_of(id).await?;
        Ok(env_view(&row, text.as_deref()))
    }

    /// The `.env` text. Masked unless `reveal` (session, or an `admin` token); revealing is audited.
    pub async fn env_content_read(
        &self,
        actor: &Actor,
        id: &str,
        reveal: bool,
    ) -> Result<String, CoreError> {
        actor.authorize(&OP_PROJECT_ENV_CONTENT_READ, Some(id))?;
        if reveal && actor.token_scope().is_some_and(|s| s < Scope::Admin) {
            return Err(CoreError::forbidden(
                "revealing environment values requires an API Token with admin scope",
            ));
        }
        let (row, text) = self.env_file_of(id).await?;
        let text = text.unwrap_or_default();
        if !reveal {
            return Ok(env::parse(&text).masked_text());
        }
        let mut tx = self.ctx.db.begin_write().await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("project.env_read", "project")
                .entity(id)
                .project(id)
                .meta(json!({ "env_file": row.env_file, "keys": env::parse(&text).keys().len() })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(text)
    }

    /// Parse arbitrary text; values are masked, nothing is stored.
    pub async fn env_preview(&self, actor: &Actor, text: &str) -> Result<EnvView, CoreError> {
        actor.authorize(&OP_PROJECT_ENV_PREVIEW, None)?;
        text_limit("content", text)?;
        let f = env::parse(text);
        Ok(EnvView {
            env_file: String::new(),
            exists: true,
            variables: mask_vars(&f),
            invalid_lines: f.invalid_lines,
        })
    }

    /// Replace the whole file. A value of `********` keeps the stored value for that key.
    pub async fn env_write(
        &self,
        actor: &Actor,
        id: &str,
        text: &str,
    ) -> Result<EnvView, CoreError> {
        actor.authorize(&OP_PROJECT_ENV_WRITE, Some(id))?;
        text_limit("content", text)?;
        let (row, existing) = self.env_file_of(id).await?;
        let old = env::parse(existing.as_deref().unwrap_or(""));
        let new = env::merge_masked(&old, text);
        if !new.invalid_lines.is_empty() {
            return Err(CoreError::validation(
                "content",
                format!("lines {:?} are not KEY=VALUE", new.invalid_lines),
            ));
        }
        self.write_env(actor, &row, &new, "replace").await
    }

    pub async fn env_variables_write(
        &self,
        actor: &Actor,
        id: &str,
        changes: &[VarChange],
    ) -> Result<EnvView, CoreError> {
        actor.authorize(&OP_PROJECT_ENV_VARIABLES_WRITE, Some(id))?;
        let (row, existing) = self.env_file_of(id).await?;
        let mut file = env::parse(existing.as_deref().unwrap_or(""));
        env::patch(&mut file, changes)?;
        self.write_env(actor, &row, &file, "patch").await
    }

    async fn write_env(
        &self,
        actor: &Actor,
        row: &ProjectRow,
        file: &EnvFile,
        mode: &str,
    ) -> Result<EnvView, CoreError> {
        let path = self.env_path(row)?;
        files::ensure_project_dir(&self.eng.data_dir, &row.slug)?;
        files::write_atomic(&path, &file.render(), 0o600)?;
        let mut tx = self.ctx.db.begin_write().await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("project.env_write", "project")
                .entity(&row.id)
                .project(&row.id)
                .meta(json!({ "mode": mode, "keys": file.keys() })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(env_view(row, Some(&file.render())))
    }

    // ---- ports --------------------------------------------------------------------

    pub async fn ports_list(&self, actor: &Actor, id: &str) -> Result<Vec<PortDto>, CoreError> {
        actor.authorize(&OP_PROJECT_PORTS_LIST, Some(id))?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        load(&mut conn, id).await?;
        Ok(projects::ports_for(&mut conn, id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Replace the declared ports: loopback only, 409 with the owner of any collision.
    pub async fn ports_set(
        &self,
        actor: &Actor,
        id: &str,
        ports: &[PortInput],
    ) -> Result<Vec<PortDto>, CoreError> {
        actor.authorize(&OP_PROJECT_PORTS_SET, Some(id))?;
        let slug = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            load(&mut conn, id).await?.slug
        };
        let mut desired = Vec::with_capacity(ports.len());
        for p in ports {
            if !matches!(p.bind_address.as_str(), "127.0.0.1" | "::1") {
                return Err(CoreError::validation(
                    "bind_address",
                    "publish on 127.0.0.1 so an Ingress Provider carries the traffic",
                ));
            }
            if !matches!(p.protocol.as_str(), "tcp" | "udp") {
                return Err(CoreError::validation(
                    "protocol",
                    "protocol must be tcp or udp",
                ));
            }
            if p.host_port == 0 || p.container_port == 0 {
                return Err(CoreError::validation("host_port", "ports must be 1-65535"));
            }
            let label = p.label.trim();
            if label.is_empty() || label.chars().count() > 64 || label.chars().any(char::is_control)
            {
                return Err(CoreError::validation(
                    "label",
                    "label must be 1-64 characters",
                ));
            }
            desired.push(DesiredPort {
                label: label.to_owned(),
                service: p.compose_service.clone().unwrap_or_default(),
                host_port: p.host_port,
                container_port: p.container_port,
                bind_address: p.bind_address.clone(),
                protocol: p.protocol.clone(),
            });
        }
        let mut labels: Vec<&str> = desired.iter().map(|d| d.label.as_str()).collect();
        labels.sort_unstable();
        labels.dedup();
        if labels.len() != desired.len() {
            return Err(CoreError::validation(
                "label",
                "labels must be unique within a project",
            ));
        }
        self.ensure_no_conflicts(id, &slug, &desired).await?;
        let now = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        reconcile_ports(&mut tx, id, &desired, now).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("project.ports_set", "project")
                .entity(id)
                .project(id)
                .meta(json!({ "ports": desired.iter().map(|d| d.host_port).collect::<Vec<_>>() })),
        )
        .await?;
        let out = projects::ports_for(&mut tx, id).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(out.into_iter().map(Into::into).collect())
    }

    // ---- deploy token -------------------------------------------------------------

    pub async fn deploy_token_reveal(&self, actor: &Actor, id: &str) -> Result<Secret, CoreError> {
        actor.authorize(&OP_PROJECT_DEPLOY_TOKEN_REVEAL, Some(id))?;
        let mut tx = self.ctx.db.begin_write().await?;
        let row = load(&mut tx, id).await?;
        let token =
            self.ctx
                .secrets
                .open("projects", "deploy_token_enc", id, &row.deploy_token_enc)?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("project.deploy_token_reveal", "project")
                .entity(id)
                .project(id),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(token)
    }

    /// New token; the old bearer and the GitHub webhook secret stop working at once.
    pub async fn deploy_token_rotate(&self, actor: &Actor, id: &str) -> Result<Secret, CoreError> {
        actor.authorize(&OP_PROJECT_DEPLOY_TOKEN_ROTATE, Some(id))?;
        let mut tx = self.ctx.db.begin_write().await?;
        let mut row = load(&mut tx, id).await?;
        let (token, hash, enc) = token_pair(&self.ctx, id)?;
        row.deploy_token_hash = hash;
        row.deploy_token_enc = enc;
        row.updated_at = self.ctx.now();
        projects::save(&mut tx, &row).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("project.deploy_token_rotate", "project")
                .entity(id)
                .project(id),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(token)
    }

    // ---- stop / restart -------------------------------------------------------------

    pub async fn stop(&self, actor: &Actor, id: &str) -> Result<StackOutput, CoreError> {
        actor.authorize(&OP_PROJECT_STOP, Some(id))?;
        self.stack_verb(actor, id, "stop", "project.stop").await
    }

    pub async fn restart(&self, actor: &Actor, id: &str) -> Result<StackOutput, CoreError> {
        actor.authorize(&OP_PROJECT_RESTART, Some(id))?;
        self.stack_verb(actor, id, "restart", "project.restart")
            .await
    }

    async fn stack_verb(
        &self,
        actor: &Actor,
        id: &str,
        verb: &'static str,
        action: &'static str,
    ) -> Result<StackOutput, CoreError> {
        let row = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            load(&mut conn, id).await?
        };
        let dir = files::project_dir(&self.eng.data_dir, &row.slug)?;
        if !files::resolve_in(&dir, &row.compose_path)?.exists() {
            return Err(CoreError::conflict(
                "the project has not been deployed yet; deploy it first",
            ));
        }
        let mut args: Vec<String> = [
            "compose",
            "-p",
            row.slug.as_str(),
            "-f",
            row.compose_path.as_str(),
        ]
        .map(str::to_owned)
        .to_vec();
        if dir.join(OVERRIDE_FILE).exists() {
            args.extend(["-f".to_owned(), OVERRIDE_FILE.to_owned()]);
        }
        if row.env_file != ".env" {
            args.extend(["--env-file".to_owned(), row.env_file.clone()]);
        }
        args.push(verb.to_owned());
        let tail = Arc::new(TailSink::default());
        let out = self
            .eng
            .host
            .command
            .run(
                CommandSpec::args_owned("docker", args)
                    .cwd(dir)
                    .timeout(std::time::Duration::from_secs(300)),
                tail.clone() as Arc<dyn LogSink>,
                &CancellationToken::new(),
            )
            .await?;
        let mut tx = self.ctx.db.begin_write().await?;
        let mut ev = AuditEvent::new(action, "project").entity(id).project(id);
        if out.exit_code != 0 {
            ev.outcome = audit::Outcome::Error;
        }
        audit::write(&self.ctx, &mut tx, &AuditActor::from_actor(actor), &ev).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(StackOutput {
            ok: out.exit_code == 0,
            output: tail.text(),
        })
    }
}

fn clean_compose(c: Option<&str>) -> Result<Option<String>, CoreError> {
    match c {
        Some(t) if !t.trim().is_empty() => {
            text_limit("compose_content", t)?;
            Ok(Some(t.replace("\r\n", "\n")))
        }
        _ => Ok(None),
    }
}

fn mask_vars(f: &EnvFile) -> Vec<EnvVariable> {
    f.keys()
        .into_iter()
        .map(|key| EnvVariable {
            key,
            value: env::MASK.to_owned(),
        })
        .collect()
}

fn env_view(row: &ProjectRow, text: Option<&str>) -> EnvView {
    let f = env::parse(text.unwrap_or(""));
    EnvView {
        env_file: row.env_file.clone(),
        exists: text.is_some(),
        variables: mask_vars(&f),
        invalid_lines: f.invalid_lines,
    }
}

/// `user:<id>`, `token:<id>` or `cli`, as stored in `deployments.triggered_by`.
pub(crate) fn triggered_by(actor: &Actor) -> String {
    match &actor.kind {
        ActorKind::User => format!("user:{}", actor.user_id.as_deref().unwrap_or("?")),
        ActorKind::ApiToken { token_id, .. } => format!("token:{token_id}"),
        ActorKind::Cli => "cli".to_owned(),
    }
}
