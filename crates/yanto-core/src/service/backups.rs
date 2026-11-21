//! Backups (docs/rewrite/04 section 7, D14, D22): the framework (rows, storage under
//! `<data_dir>/backups`, locks, free-space guard, retention, streaming download, delete) and the
//! Yanto-DB backup (SQLite online snapshot bundled with `keys/app.key`, Owner only).
//!
//! Project volume archives (one `.tar.zst` per named volume) and Postgres/MySQL dumps of the
//! databases a Project declares run through `ProjectBackupPort`; restores of both are here too.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use super::audit::{self, AuditActor, AuditEvent};
use super::{Ctx, DAY_MS};
use crate::actor::{Access, Actor, Channel, Need, OpSpec, Origin, Permission, Scope};
use crate::backup::{
    self, BackupKind, DEFAULT_KEEP_LAST, FAILED_RETENTION_DAYS, MIN_FREE_BYTES, Trigger, Written,
};
use crate::crypto::new_id;
use crate::db::repo::backups::{self as repo, BackupRow, NewBackup, SeriesKey};
use crate::db::repo::project_databases::{self as db_repo, DatabaseRow};
use crate::db::repo::{db_err, projects};
use crate::error::{CoreError, ErrorKind};
use crate::ports::{
    DbTarget, HostMgmtPorts, ProjectBackupRequest, ProjectRestoreRequest, RestoreMode,
};

pub const OP_BACKUPS_LIST: OpSpec = OpSpec::new("backups_list", Access::Authenticated, Scope::Read);
pub const OP_BACKUP_TARGETS_LIST: OpSpec =
    OpSpec::new("backup_targets_list", Access::Authenticated, Scope::Read);
pub const OP_BACKUP_CREATE: OpSpec = OpSpec::new(
    "backup_create",
    Access::Project(Need::Permission(Permission::Backups)),
    Scope::Write,
);
/// `backup_create` for `yanto_sqlite`: the archive holds `app.key`, so Owner only (03 a.2).
pub const OP_BACKUP_CREATE_YANTO: OpSpec =
    OpSpec::new("backup_create_yanto", Access::Owner, Scope::Write);
pub const OP_BACKUP_DOWNLOAD: OpSpec = OpSpec::new(
    "backup_download",
    Access::Project(Need::Permission(Permission::Backups)),
    Scope::Admin,
);
pub const OP_BACKUP_DOWNLOAD_YANTO: OpSpec =
    OpSpec::new("backup_download_yanto", Access::Owner, Scope::Admin);
pub const OP_BACKUP_DELETE: OpSpec = OpSpec::new(
    "backup_delete",
    Access::Project(Need::Permission(Permission::Backups)),
    Scope::Admin,
);
pub const OP_BACKUP_DELETE_YANTO: OpSpec =
    OpSpec::new("backup_delete_yanto", Access::Owner, Scope::Admin);
/// Restore a Project backup into its volume or declared database (04 7.6). Owner only.
pub const OP_BACKUP_RESTORE: OpSpec = OpSpec::new("backup_restore", Access::Owner, Scope::Admin);
/// `yanto backup prune`: dashboard/CLI only.
pub const OP_BACKUP_PRUNE: OpSpec =
    OpSpec::new("backup_prune", Access::Owner, Scope::Admin).session_only();

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_BACKUPS_LIST,
    &OP_BACKUP_TARGETS_LIST,
    &OP_BACKUP_CREATE,
    &OP_BACKUP_CREATE_YANTO,
    &OP_BACKUP_DOWNLOAD,
    &OP_BACKUP_DOWNLOAD_YANTO,
    &OP_BACKUP_DELETE,
    &OP_BACKUP_DELETE_YANTO,
    &OP_BACKUP_RESTORE,
    &OP_BACKUP_PRUNE,
];

/// Message of the `Conflict` returned when the series lock is held (CLI exits 75 on it).
pub const BUSY_MESSAGE: &str = "another backup of this series is already running";
const LIST_LIMIT: i64 = 200;

#[derive(Debug, Clone, Serialize)]
pub struct BackupDto {
    pub id: String,
    pub project_id: Option<String>,
    pub database_id: Option<String>,
    pub kind: String,
    pub status: String,
    pub trigger: String,
    pub filename: String,
    pub size_bytes: Option<i64>,
    pub sha256: Option<String>,
    pub error: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub downloaded_at: Option<i64>,
    pub download_count: i64,
    /// Holds secret material (the Yanto-DB archive bundles `app.key`).
    pub sensitive: bool,
}

impl From<BackupRow> for BackupDto {
    fn from(r: BackupRow) -> Self {
        Self {
            sensitive: r.kind == BackupKind::YantoSqlite.as_str(),
            id: r.id,
            project_id: r.project_id,
            database_id: r.database_id,
            kind: r.kind,
            status: r.status,
            trigger: r.trigger,
            filename: r.filename,
            size_bytes: r.size_bytes,
            sha256: r.sha256,
            error: r.error,
            note: r.note,
            created_at: r.created_at,
            finished_at: r.finished_at,
            downloaded_at: r.downloaded_at,
            download_count: r.download_count,
        }
    }
}

/// Something a backup can be created for. `target` is the volume name (`project_volumes`) or the
/// declared compose service (dump kinds); `None` for the Yanto database.
#[derive(Debug, Clone, Serialize)]
pub struct BackupTarget {
    pub kind: BackupKind,
    pub label: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub target: Option<String>,
}

/// How to restore (04 7.6). The default is a new volume; a dump always goes into the service.
#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// Empty the original volume instead of creating `<volume>-restore-<ts>`.
    pub in_place: bool,
    /// The Project name typed by the operator; required for in-place volume and database restores.
    pub confirm_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestoreOutcome {
    /// The volume the archive was extracted into.
    pub new_volume: Option<String>,
    /// The `pre_restore` backup taken first (in-place and database restores).
    pub pre_restore: Option<BackupDto>,
}

/// Scheduled-run retention flags (`--keep-last`, `--keep-days`, 7.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Retention {
    pub keep_last: Option<u32>,
    pub keep_days: Option<u32>,
}

/// A file ready to stream to the client.
#[derive(Debug, Clone)]
pub struct DownloadFile {
    pub path: PathBuf,
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub sensitive: bool,
}

/// What a backup run captures, once any Project reference is resolved.
struct Job<'a> {
    kind: BackupKind,
    /// `(project id, slug)` for Project kinds.
    project: Option<(&'a str, &'a str)>,
    target: Option<&'a str>,
    /// The declaration behind a dump kind.
    db: Option<&'a DatabaseRow>,
    stop_project: bool,
    trigger: Trigger,
}

#[derive(Clone)]
pub struct BackupService {
    ctx: Ctx,
    host: HostMgmtPorts,
    data_dir: PathBuf,
}

impl std::fmt::Debug for BackupService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackupService").finish_non_exhaustive()
    }
}

/// Project slugs and backup target names (volumes, Compose services) end up in paths, lock
/// names and `docker` arguments: one safe path component, no leading `.` or `-` (7.9).
fn safe_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with(['.', '-'])
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl BackupService {
    pub(super) fn new(ctx: Ctx, host: HostMgmtPorts, data_dir: PathBuf) -> Self {
        Self {
            ctx,
            host,
            data_dir,
        }
    }

    fn audit_actor(actor: &Actor) -> AuditActor {
        AuditActor::from_actor(actor)
    }

    /// Spec for an existing row: the Yanto-DB archive is Owner only, Project kinds need `backups`.
    fn spec_for<'s>(
        row: &BackupRow,
        project: &'s OpSpec,
        yanto: &'s OpSpec,
    ) -> (&'s OpSpec, Option<String>) {
        if row.kind == BackupKind::YantoSqlite.as_str() {
            (yanto, None)
        } else {
            (project, row.project_id.clone())
        }
    }

    async fn load(&self, id: &str) -> Result<BackupRow, CoreError> {
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        repo::get(&mut conn, id)
            .await?
            .ok_or_else(|| CoreError::not_found("backup not found"))
    }

    /// Owner and CLI see everything; a Member sees rows of Projects where they hold `backups`.
    pub async fn list(&self, actor: &Actor) -> Result<Vec<BackupDto>, CoreError> {
        actor.authorize(&OP_BACKUPS_LIST, None)?;
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        let rows = repo::list(&mut conn, LIST_LIMIT).await?;
        Ok(rows
            .into_iter()
            .filter(|r| {
                actor.is_owner()
                    || r.project_id.as_ref().is_some_and(|p| {
                        actor
                            .grants
                            .get(p)
                            .is_some_and(|perms| perms.contains(&Permission::Backups))
                    })
            })
            .map(Into::into)
            .collect())
    }

    /// What can be backed up: the Yanto database (Owner) plus each visible Project's named
    /// volumes and declared databases (needs `backups` on the Project).
    pub async fn targets(&self, actor: &Actor) -> Result<Vec<BackupTarget>, CoreError> {
        actor.authorize(&OP_BACKUP_TARGETS_LIST, None)?;
        let mut out = Vec::new();
        if actor.is_owner() {
            out.push(BackupTarget {
                kind: BackupKind::YantoSqlite,
                label: "Yanto database (with app.key)".into(),
                project_id: None,
                project_name: None,
                target: None,
            });
        }
        let rows = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            projects::list(&mut conn).await?
        };
        for p in rows {
            let allowed = actor.is_owner()
                || actor
                    .grants
                    .get(&p.id)
                    .is_some_and(|perms| perms.contains(&Permission::Backups));
            if !allowed {
                continue;
            }
            // A stopped daemon must not hide the declared databases.
            if let Ok(vols) = self.host.project_backups.list_volumes(&p.slug).await {
                for v in vols {
                    out.push(BackupTarget {
                        kind: BackupKind::ProjectVolumes,
                        label: format!("{}: volume {v}", p.name),
                        project_id: Some(p.id.clone()),
                        project_name: Some(p.name.clone()),
                        target: Some(v),
                    });
                }
            }
            let dbs = {
                let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
                db_repo::list(&mut conn, &p.id).await?
            };
            for d in dbs {
                let Some(kind) = db_kind(&d.engine) else {
                    continue;
                };
                out.push(BackupTarget {
                    kind,
                    label: format!("{}: {} service {}", p.name, d.engine, d.compose_service),
                    project_id: Some(p.id.clone()),
                    project_name: Some(p.name.clone()),
                    target: Some(d.compose_service),
                });
            }
        }
        Ok(out)
    }

    /// Back up Yanto's own database bundled with `keys/app.key` (D22). Owner only.
    pub async fn create_yanto(
        &self,
        actor: &Actor,
        trigger: Trigger,
        retention: Option<Retention>,
    ) -> Result<BackupDto, CoreError> {
        actor.authorize(&OP_BACKUP_CREATE_YANTO, None)?;
        let job = Job {
            kind: BackupKind::YantoSqlite,
            project: None,
            target: None,
            stop_project: false,
            db: None,
            trigger,
        };
        let dto = self.run(&Self::audit_actor(actor), &job).await?;
        if trigger == Trigger::Cron {
            let who = Self::audit_actor(actor);
            let key = series_key_of(&dto, BackupKind::YantoSqlite);
            self.apply_retention(&who, &key, Trigger::Cron, retention.unwrap_or_default())
                .await?;
            self.purge_failed(&who).await?;
        }
        Ok(dto)
    }

    /// Manual backup of a Project's volumes (`target` = one volume, `None` = every named volume,
    /// one archive each) or of one declared database (`target` = its compose service).
    pub async fn create_project(
        &self,
        actor: &Actor,
        project_id: &str,
        kind: BackupKind,
        target: Option<&str>,
        stop_project: bool,
    ) -> Result<Vec<BackupDto>, CoreError> {
        self.create_project_with(
            actor,
            project_id,
            kind,
            target,
            stop_project,
            Trigger::Manual,
            None,
        )
        .await
    }

    /// [`Self::create_project`] with an explicit trigger; a `cron` run applies `retention` to each
    /// series it wrote (04 7.4).
    #[allow(clippy::too_many_arguments)]
    pub async fn create_project_with(
        &self,
        actor: &Actor,
        project_id: &str,
        kind: BackupKind,
        target: Option<&str>,
        stop_project: bool,
        trigger: Trigger,
        retention: Option<Retention>,
    ) -> Result<Vec<BackupDto>, CoreError> {
        actor.authorize(&OP_BACKUP_CREATE, Some(project_id))?;
        if !kind.is_project() {
            return Err(CoreError::validation(
                "kind",
                "use the Yanto database backup for yanto_sqlite",
            ));
        }
        let slug = self.project_slug(project_id).await?;
        let who = Self::audit_actor(actor);
        let mut out = Vec::new();
        if kind == BackupKind::ProjectVolumes {
            let volumes = match target {
                Some(v) => vec![v.to_owned()],
                None => self.host.project_backups.list_volumes(&slug).await?,
            };
            if volumes.is_empty() {
                return Err(CoreError::validation(
                    "target",
                    "this Project has no named volumes (bind mounts are not backed up)",
                ));
            }
            let mut first_err = None;
            for v in &volumes {
                let job = Job {
                    kind,
                    project: Some((project_id, &slug)),
                    target: Some(v),
                    db: None,
                    stop_project,
                    trigger,
                };
                match self.run(&who, &job).await {
                    Ok(b) => out.push(b),
                    Err(e) => {
                        first_err.get_or_insert(e);
                    }
                }
            }
            if let Some(e) = first_err {
                return Err(e);
            }
        } else {
            let service = target.ok_or_else(|| {
                CoreError::validation("target", "name the declared database service")
            })?;
            let decl = {
                let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
                db_repo::by_service(&mut conn, project_id, service).await?
            }
            .ok_or_else(|| {
                CoreError::validation(
                    "target",
                    "no database is declared for that service; declare it on the Project first",
                )
            })?;
            if db_kind(&decl.engine) != Some(kind) {
                return Err(CoreError::validation(
                    "kind",
                    format!("that service is declared as {}", decl.engine),
                ));
            }
            let job = Job {
                kind,
                project: Some((project_id, &slug)),
                target: Some(service),
                db: Some(&decl),
                stop_project: false,
                trigger,
            };
            out.push(self.run(&who, &job).await?);
        }
        if trigger == Trigger::Cron {
            for b in &out {
                let key = series_key_of(b, kind);
                self.apply_retention(&who, &key, Trigger::Cron, retention.unwrap_or_default())
                    .await?;
            }
            self.purge_failed(&who).await?;
        }
        Ok(out)
    }

    async fn project_slug(&self, project_id: &str) -> Result<String, CoreError> {
        let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
        projects::get(&mut conn, project_id)
            .await?
            .map(|p| p.slug)
            .ok_or_else(|| CoreError::not_found("project not found"))
    }

    fn series_dir(&self, job: &Job<'_>) -> Result<PathBuf, CoreError> {
        match job.project {
            None => Ok(backup::yanto_dir(&self.data_dir)),
            Some((_, slug)) if safe_component(slug) => Ok(backup::backups_dir(&self.data_dir)
                .join("projects")
                .join(slug)),
            Some(_) => Err(CoreError::validation("project", "invalid Project slug")),
        }
    }

    /// Create the directory chain under `backups/` with `0700` on every level.
    fn ensure_dirs(&self, dir: &Path) -> Result<(), CoreError> {
        let root = backup::backups_dir(&self.data_dir);
        backup::ensure_private_dir(&root)?;
        let rel = dir.strip_prefix(&root).unwrap_or(Path::new(""));
        let mut cur = root;
        for c in rel.components() {
            cur.push(c);
            backup::ensure_private_dir(&cur)?;
        }
        Ok(())
    }

    /// The framework: lock, guard, row, work, finalize, audit.
    async fn run(&self, who: &AuditActor, job: &Job<'_>) -> Result<BackupDto, CoreError> {
        let series = match job.project {
            None => "yanto".to_owned(),
            Some((_, slug)) => format!(
                "{slug}-{}-{}",
                job.kind.as_str(),
                job.target.unwrap_or("all")
            ),
        };
        if let Some(t) = job.target
            && !safe_component(t)
        {
            return Err(CoreError::validation("target", "invalid target name"));
        }
        let lock_name = format!("backup-{series}");
        let _lock = self
            .host
            .backup_host
            .try_lock(&lock_name)?
            .ok_or_else(|| CoreError::conflict(BUSY_MESSAGE))?;

        if job.kind == BackupKind::YantoSqlite
            && !crate::secrets::key_path(&self.data_dir).is_file()
        {
            return Err(CoreError::validation(
                "app.key",
                "keys/app.key is missing; a backup without it could not be restored",
            ));
        }
        let dir = self.series_dir(job)?;
        self.ensure_dirs(&dir)?;
        self.free_space_guard(&dir, job.kind).await?;

        let now = self.ctx.now();
        let ts = backup::timestamp(now);
        let suffix = job.kind.file_suffix();
        let mut filename = format!("{ts}{suffix}");
        let mut n = 1;
        while dir.join(&filename).exists() || dir.join(format!(".{filename}.partial")).exists() {
            n += 1;
            filename = format!("{ts}-{n}{suffix}");
        }
        let final_path = dir.join(&filename);
        let partial = dir.join(format!(".{filename}.partial"));
        let db_tmp = dir.join(format!(".{filename}.db.partial"));
        let id = new_id("bak");
        // A volume archive's series is identified by the volume name (no database row).
        let volume_note = (job.kind == BackupKind::ProjectVolumes)
            .then(|| job.target.map(|t| format!("volume={t}")))
            .flatten();
        {
            let mut tx = self.ctx.db.begin_write().await?;
            repo::insert_running(
                &mut tx,
                &NewBackup {
                    id: &id,
                    project_id: job.project.map(|p| p.0),
                    kind: job.kind.as_str(),
                    database_id: job.db.map(|d| d.id.as_str()),
                    trigger: job.trigger.as_str(),
                    filename: &filename,
                    file_path: &final_path.to_string_lossy(),
                    note: volume_note.as_deref(),
                    now,
                },
            )
            .await?;
            tx.commit().await.map_err(db_err)?;
        }

        let built = self.build(job, &partial, &db_tmp).await;
        let written = match built.and_then(|w| {
            std::fs::rename(&partial, &final_path)
                .map_err(|e| CoreError::internal("cannot finalize the backup file", e))?;
            backup::write_sidecar(&final_path, &w.sha256)?;
            Ok(w)
        }) {
            Ok(w) => w,
            Err(e) => {
                cleanup(&[&partial, &db_tmp, &final_path]);
                let finished = self.ctx.now();
                let mut tx = self.ctx.db.begin_write().await?;
                repo::mark_failed(&mut tx, &id, &e.message, finished).await?;
                audit::write(
                    &self.ctx,
                    &mut tx,
                    who,
                    &AuditEvent::new("backup.create", "backup")
                        .entity(&id)
                        .meta(json!({ "kind": job.kind.as_str(), "error": e.message })),
                )
                .await?;
                tx.commit().await.map_err(db_err)?;
                return Err(e);
            }
        };
        let finished = self.ctx.now();
        let mut tx = self.ctx.db.begin_write().await?;
        repo::mark_success(
            &mut tx,
            &id,
            i64::try_from(written.size_bytes).unwrap_or(i64::MAX),
            &written.sha256,
            finished,
        )
        .await?;
        let mut ev = AuditEvent::new("backup.create", "backup").entity(&id).meta(json!({
            "kind": job.kind.as_str(), "trigger": job.trigger.as_str(),
            "size_bytes": written.size_bytes, "sha256": written.sha256, "sensitive": job.kind == BackupKind::YantoSqlite,
        }));
        if let Some((pid, _)) = job.project {
            ev = ev.project(pid);
        }
        audit::write(&self.ctx, &mut tx, who, &ev).await?;
        let row = repo::get(&mut tx, &id)
            .await?
            .ok_or_else(|| CoreError::not_found("backup not found"))?;
        tx.commit().await.map_err(db_err)?;
        Ok(row.into())
    }

    async fn free_space_guard(&self, dir: &Path, kind: BackupKind) -> Result<(), CoreError> {
        let prev = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            repo::latest_success(&mut conn, kind.as_str())
                .await?
                .and_then(|r| r.size_bytes)
                .and_then(|s| u64::try_from(s).ok())
                .unwrap_or(0)
        };
        let need = MIN_FREE_BYTES.max(prev.saturating_mul(2));
        match self.host.backup_host.free_bytes(dir) {
            Some(free) if free < need => Err(CoreError::new(
                ErrorKind::Unavailable,
                format!(
                    "not enough free disk space for a backup ({} MiB free, {} MiB needed)",
                    free / (1024 * 1024),
                    need / (1024 * 1024)
                ),
            )),
            _ => Ok(()),
        }
    }

    /// Produce the archive at `partial`. Blocking work runs off the async threads.
    async fn build(
        &self,
        job: &Job<'_>,
        partial: &Path,
        db_tmp: &Path,
    ) -> Result<Written, CoreError> {
        match job.kind {
            BackupKind::YantoSqlite => {
                backup::snapshot_db(&self.ctx.db, db_tmp).await?;
                backup::integrity_check(db_tmp).await?;
                for ext in ["-wal", "-shm"] {
                    let mut p = db_tmp.as_os_str().to_owned();
                    p.push(ext);
                    let _ = std::fs::remove_file(p);
                }
                let (db, key, dest) = (
                    db_tmp.to_path_buf(),
                    crate::secrets::key_path(&self.data_dir),
                    partial.to_path_buf(),
                );
                let mtime = u64::try_from(self.ctx.now() / 1000).unwrap_or(0);
                let w = tokio::task::spawn_blocking(move || {
                    backup::write_yanto_archive(&db, &key, &dest, mtime)
                })
                .await
                .map_err(|e| CoreError::internal("backup task failed", e))??;
                let _ = std::fs::remove_file(db_tmp);
                Ok(w)
            }
            _ => {
                let (_, slug) = job.project.ok_or_else(|| {
                    CoreError::new(ErrorKind::Internal, "Project backup without a Project")
                })?;
                let req = ProjectBackupRequest {
                    kind: job.kind,
                    project_slug: slug.to_owned(),
                    target: job.target.map(str::to_owned),
                    db: job.db.map(db_target),
                    stop_project: job.stop_project,
                };
                self.host.project_backups.create(&req, partial).await?;
                let p = partial.to_path_buf();
                tokio::task::spawn_blocking(move || {
                    let sha256 = backup::sha256_file(&p)?;
                    let size_bytes = std::fs::metadata(&p)
                        .map_err(|e| CoreError::internal("cannot stat the backup", e))?
                        .len();
                    Ok(Written { size_bytes, sha256 })
                })
                .await
                .map_err(|e| CoreError::internal("backup task failed", e))?
            }
        }
    }

    /// Delete the backups of `key` made by `trigger` beyond the policy (04 7.4). The newest
    /// successful one is never pruned.
    async fn apply_retention(
        &self,
        who: &AuditActor,
        key: &SeriesKey,
        trigger: Trigger,
        policy: Retention,
    ) -> Result<u64, CoreError> {
        let policy = if trigger == Trigger::PreRestore {
            Retention {
                keep_last: Some(PRE_RESTORE_KEEP),
                keep_days: None,
            }
        } else if policy == Retention::default() {
            Retention {
                keep_last: Some(DEFAULT_KEEP_LAST),
                keep_days: None,
            }
        } else {
            policy
        };
        let now = self.ctx.now();
        let series = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            repo::series(&mut conn, key, trigger.as_str()).await?
        };
        let mut doomed: Vec<BackupRow> = Vec::new();
        for (i, r) in series.into_iter().enumerate() {
            if i == 0 {
                continue;
            }
            let too_many = policy.keep_last.is_some_and(|n| i >= n as usize);
            let too_old = policy
                .keep_days
                .is_some_and(|d| r.created_at < now - i64::from(d) * DAY_MS);
            if too_many || too_old {
                doomed.push(r);
            }
        }
        self.delete_rows(who, &doomed, &key.kind).await
    }

    /// Failed rows older than 7 days (04 7.4).
    async fn purge_failed(&self, who: &AuditActor) -> Result<u64, CoreError> {
        let failed = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            repo::old_failed(&mut conn, self.ctx.now() - FAILED_RETENTION_DAYS * DAY_MS).await?
        };
        self.delete_rows(who, &failed, "failed").await
    }

    async fn delete_rows(
        &self,
        who: &AuditActor,
        rows: &[BackupRow],
        what: &str,
    ) -> Result<u64, CoreError> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut tx = self.ctx.db.begin_write().await?;
        for r in rows {
            self.remove_files(r);
            repo::delete(&mut tx, &r.id).await?;
        }
        audit::write(
            &self.ctx,
            &mut tx,
            who,
            &AuditEvent::new("backup.prune", "backup")
                .meta(json!({ "kind": what, "deleted": rows.len() })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(rows.len() as u64)
    }

    /// `yanto backup prune`: apply the retention flags to every scheduled series (the Yanto
    /// database and each Project's volumes and databases), trim `pre_restore` backups to the
    /// newest three per series and drop old failed rows.
    pub async fn prune(&self, actor: &Actor, retention: Retention) -> Result<u64, CoreError> {
        actor.authorize(&OP_BACKUP_PRUNE, None)?;
        let who = Self::audit_actor(actor);
        let (cron_keys, pre_keys) = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            (
                repo::series_keys(&mut conn, Trigger::Cron.as_str()).await?,
                repo::series_keys(&mut conn, Trigger::PreRestore.as_str()).await?,
            )
        };
        let mut n = 0;
        for k in &cron_keys {
            n += self
                .apply_retention(&who, k, Trigger::Cron, retention)
                .await?;
        }
        for k in &pre_keys {
            n += self
                .apply_retention(&who, k, Trigger::PreRestore, retention)
                .await?;
        }
        n += self.purge_failed(&who).await?;
        Ok(n)
    }

    /// Remove the archive and its checksum sidecar, only when they live under `backups/`.
    fn remove_files(&self, row: &BackupRow) {
        let path = Path::new(&row.file_path);
        if let Ok(real) = backup::within_backups(&self.data_dir, path) {
            let _ = std::fs::remove_file(backup::sidecar_path(&real));
            let _ = std::fs::remove_file(real);
        }
    }

    pub async fn delete(&self, actor: &Actor, id: &str) -> Result<(), CoreError> {
        let row = self.load(id).await?;
        let (spec, project) = Self::spec_for(&row, &OP_BACKUP_DELETE, &OP_BACKUP_DELETE_YANTO);
        actor.authorize(spec, project.as_deref())?;
        if row.status == "running" {
            return Err(CoreError::conflict("a running backup cannot be deleted"));
        }
        let mut tx = self.ctx.db.begin_write().await?;
        self.remove_files(&row);
        repo::delete(&mut tx, id).await?;
        let mut ev = AuditEvent::new("backup.delete", "backup")
            .entity(id)
            .meta(json!({ "kind": row.kind, "filename": row.filename }));
        if let Some(p) = &row.project_id {
            ev = ev.project(p);
        }
        audit::write(&self.ctx, &mut tx, &Self::audit_actor(actor), &ev).await?;
        tx.commit().await.map_err(db_err)
    }

    /// Restore a Project backup (04 7.6). Volumes go into a new `<volume>-restore-<ts>` volume
    /// unless `in_place` is set; dumps go into the declared service. The archive's checksum is
    /// verified before anything changes, and in-place and database restores take a `pre_restore`
    /// backup of the current state first (a failure there aborts the restore).
    pub async fn restore(
        &self,
        actor: &Actor,
        id: &str,
        opts: RestoreOptions,
    ) -> Result<RestoreOutcome, CoreError> {
        actor.authorize(&OP_BACKUP_RESTORE, None)?;
        let row = self.load(id).await?;
        if row.status != "success" {
            return Err(CoreError::conflict(
                "only completed backups can be restored",
            ));
        }
        let kind = BackupKind::parse(&row.kind)
            .filter(|k| k.is_project())
            .ok_or_else(|| {
                CoreError::validation(
                    "backup",
                    "the Yanto database is restored from the CLI: yanto restore yanto <archive>",
                )
            })?;
        let project_id = row.project_id.clone().ok_or_else(|| {
            CoreError::validation("backup", "the Project of this backup no longer exists")
        })?;
        let (slug, name) = {
            let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
            let p = projects::get(&mut conn, &project_id)
                .await?
                .ok_or_else(|| CoreError::not_found("project not found"))?;
            (p.slug, p.name)
        };
        let decl = match &row.database_id {
            Some(did) => {
                let mut conn = self.ctx.db.reader().acquire().await.map_err(db_err)?;
                db_repo::get(&mut conn, did).await?
            }
            None => None,
        };
        let volume = row
            .note
            .as_deref()
            .and_then(|n| n.strip_prefix("volume="))
            .map(str::to_owned);
        let (target, db) = match kind {
            BackupKind::ProjectVolumes => {
                let v = volume.ok_or_else(|| {
                    CoreError::validation("backup", "this archive does not name its volume")
                })?;
                (v, None)
            }
            _ => {
                let d = decl.ok_or_else(|| {
                    CoreError::validation(
                        "backup",
                        "the database declaration of this backup was removed; declare it again",
                    )
                })?;
                (d.compose_service.clone(), Some(d))
            }
        };
        let destructive = opts.in_place || kind != BackupKind::ProjectVolumes;
        if destructive && opts.confirm_name.as_deref().map(str::trim) != Some(name.as_str()) {
            return Err(CoreError::validation(
                "confirm",
                "type the Project name to confirm",
            ));
        }
        if !safe_component(&slug) || !safe_component(&target) {
            return Err(CoreError::validation("backup", "invalid Project or target"));
        }
        // Checksum before any change.
        let file = backup::within_backups(&self.data_dir, Path::new(&row.file_path))?;
        let want = row.sha256.clone().unwrap_or_default();
        let have = {
            let f = file.clone();
            tokio::task::spawn_blocking(move || backup::sha256_file(&f))
                .await
                .map_err(|e| CoreError::internal("checksum task failed", e))??
        };
        if !crate::crypto::ct_eq(&have, &want) {
            return Err(CoreError::validation(
                "backup",
                "checksum mismatch: the file changed since it was written; nothing was restored",
            ));
        }
        let _lock = self
            .host
            .backup_host
            .try_lock(&format!("restore-{slug}"))?
            .ok_or_else(|| CoreError::conflict("a restore of this Project is already running"))?;

        let who = Self::audit_actor(actor);
        let mut pre_restore = None;
        let mode = if destructive {
            let job = Job {
                kind,
                project: Some((&project_id, &slug)),
                target: Some(&target),
                db: db.as_ref(),
                stop_project: false,
                trigger: Trigger::PreRestore,
            };
            let pre = self.run(&who, &job).await.map_err(|e| {
                CoreError::new(
                    e.kind.clone(),
                    format!(
                        "pre-restore backup failed, nothing was restored: {}",
                        e.message
                    ),
                )
            })?;
            let key = series_key_of(&pre, kind);
            self.apply_retention(&who, &key, Trigger::PreRestore, Retention::default())
                .await?;
            pre_restore = Some(pre);
            RestoreMode::InPlace
        } else {
            RestoreMode::NewVolume(format!(
                "{target}-restore-{}",
                backup::timestamp(self.ctx.now()).to_ascii_lowercase()
            ))
        };
        let req = ProjectRestoreRequest {
            kind,
            project_slug: slug.clone(),
            target: Some(target.clone()),
            db: db.as_ref().map(db_target),
            mode: mode.clone(),
        };
        let result = self.host.project_backups.restore(&req, &file).await;
        let mut tx = self.ctx.db.begin_write().await?;
        let mut meta = json!({
            "kind": row.kind, "target": target, "filename": row.filename,
            "in_place": mode == RestoreMode::InPlace,
            "pre_restore": pre_restore.as_ref().map(|p| p.id.as_str()),
        });
        let mut ev = AuditEvent::new("backup.restore", "backup")
            .entity(id)
            .project(&project_id);
        if let Err(e) = &result {
            meta["error"] = json!(e.message);
            ev.outcome = audit::Outcome::Error;
        }
        audit::write(&self.ctx, &mut tx, &who, &ev.meta(meta)).await?;
        tx.commit().await.map_err(db_err)?;
        result?;
        Ok(RestoreOutcome {
            new_volume: match mode {
                RestoreMode::NewVolume(n) => Some(n),
                RestoreMode::InPlace => None,
            },
            pre_restore,
        })
    }

    /// Authorize, audit and count a download, returning the file for the adapter to stream.
    pub async fn download(&self, actor: &Actor, id: &str) -> Result<DownloadFile, CoreError> {
        let row = self.load(id).await?;
        let (spec, project) = Self::spec_for(&row, &OP_BACKUP_DOWNLOAD, &OP_BACKUP_DOWNLOAD_YANTO);
        actor.authorize(spec, project.as_deref())?;
        if row.status != "success" {
            return Err(CoreError::conflict(
                "only completed backups can be downloaded",
            ));
        }
        let path = backup::within_backups(&self.data_dir, Path::new(&row.file_path))?;
        let size = std::fs::metadata(&path)
            .map_err(|_| CoreError::not_found("backup file is missing"))?
            .len();
        let mut tx = self.ctx.db.begin_write().await?;
        repo::record_download(&mut tx, id, self.ctx.now()).await?;
        let sensitive = row.kind == BackupKind::YantoSqlite.as_str();
        let mut ev = AuditEvent::new("backup.download", "backup")
            .entity(id)
            .meta(json!({ "kind": row.kind, "filename": row.filename, "sensitive": sensitive }));
        if let Some(p) = &row.project_id {
            ev = ev.project(p);
        }
        audit::write(&self.ctx, &mut tx, &Self::audit_actor(actor), &ev).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(DownloadFile {
            path,
            filename: row.filename,
            size_bytes: size,
            sha256: row.sha256.unwrap_or_default(),
            sensitive,
        })
    }

    /// Startup recovery (7.8): `running` rows become `failed` ("interrupted") and their
    /// `.partial` files, plus any stray ones, are removed. Returns the number of rows fixed.
    pub async fn recover_interrupted(&self) -> Result<u64, CoreError> {
        let mut tx = self.ctx.db.begin_write().await?;
        let rows = repo::fail_running(&mut tx, self.ctx.now()).await?;
        if !rows.is_empty() {
            audit::write(
                &self.ctx,
                &mut tx,
                &AuditActor::system(&Origin::new(Channel::System, None)),
                &AuditEvent::new("backup.recover", "backup")
                    .meta(json!({ "interrupted": rows.len() })),
            )
            .await?;
        }
        tx.commit().await.map_err(db_err)?;
        let root = backup::backups_dir(&self.data_dir);
        sweep_partials(&root);
        Ok(rows.len() as u64)
    }
}

const PRE_RESTORE_KEEP: u32 = 3;

fn db_kind(engine: &str) -> Option<BackupKind> {
    match engine {
        "postgres" => Some(BackupKind::ProjectPostgres),
        "mysql" => Some(BackupKind::ProjectMysql),
        _ => None,
    }
}

fn db_target(d: &DatabaseRow) -> DbTarget {
    DbTarget {
        compose_service: d.compose_service.clone(),
        database: d.database.clone(),
        user_env: d.user_env.clone(),
        password_env: d.password_env.clone(),
    }
}

fn series_key_of(b: &BackupDto, kind: BackupKind) -> SeriesKey {
    SeriesKey {
        kind: kind.as_str().to_owned(),
        project_id: b.project_id.clone(),
        database_id: b.database_id.clone(),
        note: b.note.clone(),
    }
}

fn cleanup(paths: &[&Path]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(backup::sidecar_path(p));
    }
}

/// Remove `.*.partial` files under the backups tree (at most two levels below `projects/`).
fn sweep_partials(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            sweep_partials(&p);
        } else if name.starts_with('.') && name.ends_with(".partial") {
            let _ = std::fs::remove_file(p);
        }
    }
}
