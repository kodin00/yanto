//! Cron Entries (docs/rewrite/04 section 5): CRUD over `cron_entries`, the generated
//! `/etc/cron.d/yanto`, drift detection, run history, and `yanto cron-run` (D26).
//!
//! Every mutation re-renders the whole file inside the DB transaction; when the write fails the
//! transaction rolls back, so the DB and the file never disagree silently.

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;

use super::Ctx;
use super::audit::{self, AuditActor, AuditEvent};
use crate::actor::{Access, Actor, OpSpec, Scope};
use crate::cron::{
    self, EntryInput, KEEP_RUN_DAYS, KEEP_RUNS, RenderEntry, Schedule, ValidEntry, output_tail,
};
use crate::crypto::{new_id, sha256_hex};
use crate::db::repo::cron as repo;
use crate::db::repo::{db_err, settings};
use crate::error::CoreError;
use crate::ports::{CronSource, ExecOutcome, ExecSpec, HostMgmtPorts, ManagedFile};

pub const OP_CRON_LIST: OpSpec = OpSpec::new("cron_list", Access::Owner, Scope::Read);
pub const OP_CRON_FILES_LIST: OpSpec = OpSpec::new("cron_files_list", Access::Owner, Scope::Read);
pub const OP_CRON_RUNS_LIST: OpSpec = OpSpec::new("cron_runs_list", Access::Owner, Scope::Read);
pub const OP_CRON_PREVIEW: OpSpec = OpSpec::new("cron_preview", Access::Owner, Scope::Read);
pub const OP_CRON_CREATE: OpSpec = OpSpec::new("cron_create", Access::Owner, Scope::Admin);
pub const OP_CRON_UPDATE: OpSpec = OpSpec::new("cron_update", Access::Owner, Scope::Admin);
pub const OP_CRON_TOGGLE: OpSpec = OpSpec::new("cron_toggle", Access::Owner, Scope::Write);
pub const OP_CRON_DELETE: OpSpec = OpSpec::new("cron_delete", Access::Owner, Scope::Admin);
pub const OP_CRON_REAPPLY: OpSpec = OpSpec::new("cron_reapply", Access::Owner, Scope::Admin);
/// `yanto cron-run <id>`: invoked by the cron daemon as root; never through a token.
pub const OP_CRON_RUN: OpSpec = OpSpec::new("cron_run", Access::Owner, Scope::Admin).session_only();

pub const OPERATIONS: &[&OpSpec] = &[
    &OP_CRON_LIST,
    &OP_CRON_FILES_LIST,
    &OP_CRON_RUNS_LIST,
    &OP_CRON_PREVIEW,
    &OP_CRON_CREATE,
    &OP_CRON_UPDATE,
    &OP_CRON_TOGGLE,
    &OP_CRON_DELETE,
    &OP_CRON_REAPPLY,
    &OP_CRON_RUN,
];

const HASH_KEY: &str = "cron.file_sha256";
const RUNS_PAGE: i64 = 50;

#[derive(Debug, Clone, Serialize)]
pub struct CronRunDto {
    pub id: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub output_tail: Option<String>,
}

impl From<repo::CronRunRow> for CronRunDto {
    fn from(r: repo::CronRunRow) -> Self {
        Self {
            duration_ms: r.finished_at.map(|f| f - r.started_at),
            id: r.id,
            status: r.status,
            exit_code: r.exit_code,
            started_at: r.started_at,
            finished_at: r.finished_at,
            output_tail: r.output_tail,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CronEntryDto {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub run_as_user: String,
    pub command: String,
    pub overlap: String,
    pub timeout_s: Option<i64>,
    pub enabled: bool,
    pub project_id: Option<String>,
    /// The Unix user no longer exists on the Host: runs fail until the entry is edited (5.5).
    pub invalid_user: bool,
    pub next_run: Option<i64>,
    pub last_run: Option<CronRunDto>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Where the file on disk stands relative to the DB (5.4 step 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FileStatus {
    InSync,
    /// Edited outside Yanto, or absent while entries exist.
    Drift {
        missing: bool,
    },
    /// Symlink or wrong owner/mode; writes are refused until the Owner replaces it.
    Unsafe {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct CronOverview {
    pub entries: Vec<CronEntryDto>,
    pub file: FileStatus,
    pub file_path: &'static str,
    /// `false` when the cron daemon is inactive; `None` when unknown.
    pub service_active: Option<bool>,
    pub login_users: Vec<String>,
    /// Times are computed in UTC.
    pub timezone: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct SchedulePreview {
    pub canonical: String,
    pub next_runs: Vec<i64>,
    /// Both day fields restricted: cron fires when either matches.
    pub day_fields_or: bool,
    pub timezone: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub run_id: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub duration_ms: i64,
}

/// Fields to change in [`CronService::patch`]; `None` keeps the stored value.
#[derive(Debug, Clone, Default)]
pub struct CronPatch {
    pub name: Option<String>,
    pub schedule: Option<String>,
    pub run_as_user: Option<String>,
    pub command: Option<String>,
    pub overlap: Option<cron::Overlap>,
    pub timeout_s: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Clone)]
pub struct CronService {
    ctx: Ctx,
    host: HostMgmtPorts,
}

impl std::fmt::Debug for CronService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CronService").finish_non_exhaustive()
    }
}

fn fields(v: &ValidEntry) -> repo::EntryFields<'_> {
    repo::EntryFields {
        name: &v.name,
        schedule: v.schedule.as_str(),
        run_as_user: &v.run_as_user,
        command: &v.command,
        overlap: v.overlap.as_str(),
        timeout_s: v.timeout_s,
        enabled: v.enabled,
    }
}

fn audit_meta(v: &ValidEntry) -> serde_json::Value {
    // The full command is recorded on purpose (5.6); the UI warns not to put secrets in it.
    json!({
        "name": v.name, "schedule": v.schedule.as_str(), "run_as_user": v.run_as_user,
        "command": v.command, "overlap": v.overlap.as_str(), "timeout_s": v.timeout_s,
        "enabled": v.enabled,
    })
}

impl CronService {
    pub(super) fn new(ctx: Ctx, host: HostMgmtPorts) -> Self {
        Self { ctx, host }
    }

    async fn reader(&self) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, CoreError> {
        self.ctx.db.reader().acquire().await.map_err(db_err)
    }

    /// Validate input and check the Unix user exists on the Host.
    fn validate(&self, input: &EntryInput) -> Result<ValidEntry, CoreError> {
        let v = cron::validate_entry(input)?;
        match self.host.users.lookup(&v.run_as_user)? {
            Some(_) => Ok(v),
            None => Err(CoreError::validation(
                "run_as_user",
                format!("Unix user {:?} does not exist on this Host", v.run_as_user),
            )),
        }
    }

    /// Render from the rows visible on `conn`, write the file, remember its hash.
    async fn apply(
        &self,
        conn: &mut sqlx::SqliteConnection,
        replace_unsafe: bool,
    ) -> Result<(), CoreError> {
        let rows = repo::list(conn).await?;
        let entries: Vec<RenderEntry<'_>> = rows
            .iter()
            .map(|r| RenderEntry {
                id: &r.id,
                name: &r.name,
                schedule: &r.schedule,
                enabled: r.enabled,
            })
            .collect();
        let bin = self.host.cron.yanto_binary()?;
        let text = cron::render_file(&entries, &bin)?;
        self.host.cron.write_managed(&text, replace_unsafe)?;
        settings::set(
            conn,
            HASH_KEY,
            &json!(sha256_hex(&text)).to_string(),
            self.ctx.now(),
        )
        .await
    }

    pub async fn list(&self, actor: &Actor) -> Result<CronOverview, CoreError> {
        actor.authorize(&OP_CRON_LIST, None)?;
        let mut conn = self.reader().await?;
        let rows = repo::list(&mut conn).await?;
        let now = self.ctx.now();
        let mut entries = Vec::with_capacity(rows.len());
        for r in rows {
            let last = repo::runs_list(&mut conn, &r.id, 1).await?.pop();
            let invalid_user = self.host.users.lookup(&r.run_as_user)?.is_none();
            let next_run = Schedule::parse(&r.schedule)
                .ok()
                .filter(|_| r.enabled)
                .and_then(|s| s.next_runs(now, 1).first().copied());
            entries.push(CronEntryDto {
                id: r.id,
                name: r.name,
                schedule: r.schedule,
                run_as_user: r.run_as_user,
                command: r.command,
                overlap: r.overlap,
                timeout_s: r.timeout_s,
                enabled: r.enabled,
                project_id: r.project_id,
                invalid_user,
                next_run,
                last_run: last.map(Into::into),
                created_at: r.created_at,
                updated_at: r.updated_at,
            });
        }
        let stored = settings::get(&mut conn, HASH_KEY)
            .await?
            .and_then(|s| serde_json::from_str::<String>(&s).ok());
        let file = match self.host.cron.read_managed()? {
            ManagedFile::Unsafe(reason) => FileStatus::Unsafe { reason },
            ManagedFile::Missing if stored.is_none() && entries.is_empty() => FileStatus::InSync,
            ManagedFile::Missing => FileStatus::Drift { missing: true },
            ManagedFile::Present(text) => {
                if stored.as_deref() == Some(sha256_hex(&text).as_str()) {
                    FileStatus::InSync
                } else {
                    FileStatus::Drift { missing: false }
                }
            }
        };
        let login_users = self
            .host
            .users
            .login_users()?
            .into_iter()
            .map(|u| u.name)
            .collect();
        Ok(CronOverview {
            entries,
            file,
            file_path: "/etc/cron.d/yanto",
            service_active: self.host.cron.service_active(),
            login_users,
            timezone: "UTC",
        })
    }

    /// One entry (authorized like `cron_list`).
    pub async fn get(&self, actor: &Actor, id: &str) -> Result<CronEntryDto, CoreError> {
        actor.authorize(&OP_CRON_LIST, None)?;
        let mut dto = self.entry_dto(id).await?;
        let mut conn = self.reader().await?;
        dto.last_run = repo::runs_list(&mut conn, id, 1)
            .await?
            .pop()
            .map(Into::into);
        Ok(dto)
    }

    pub async fn files_list(&self, actor: &Actor) -> Result<Vec<CronSource>, CoreError> {
        actor.authorize(&OP_CRON_FILES_LIST, None)?;
        self.host.cron.system_sources()
    }

    pub async fn runs_list(
        &self,
        actor: &Actor,
        entry_id: &str,
    ) -> Result<Vec<CronRunDto>, CoreError> {
        actor.authorize(&OP_CRON_RUNS_LIST, None)?;
        let mut conn = self.reader().await?;
        if repo::get(&mut conn, entry_id).await?.is_none() {
            return Err(CoreError::not_found("Cron Entry not found"));
        }
        Ok(repo::runs_list(&mut conn, entry_id, RUNS_PAGE)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// "Next 5 run times" for a schedule being typed (5.3).
    pub async fn preview(
        &self,
        actor: &Actor,
        schedule: &str,
        count: usize,
    ) -> Result<SchedulePreview, CoreError> {
        actor.authorize(&OP_CRON_PREVIEW, None)?;
        let s = Schedule::parse(schedule).map_err(|m| CoreError::validation("schedule", m))?;
        Ok(SchedulePreview {
            canonical: s.as_str().to_owned(),
            next_runs: s.next_runs(self.ctx.now(), count.clamp(1, 20)),
            day_fields_or: s.day_fields_use_or(),
            timezone: "UTC",
        })
    }

    pub async fn create(
        &self,
        actor: &Actor,
        input: &EntryInput,
    ) -> Result<CronEntryDto, CoreError> {
        actor.authorize(&OP_CRON_CREATE, None)?;
        let v = self.validate(input)?;
        let id = new_id("crn");
        let mut tx = self.ctx.db.begin_write().await?;
        repo::insert(&mut tx, &id, &fields(&v), self.ctx.now()).await?;
        self.apply(&mut tx, false).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cron.create", "cron_entry")
                .entity(&id)
                .meta(audit_meta(&v)),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        self.entry_dto(&id).await
    }

    pub async fn update(
        &self,
        actor: &Actor,
        id: &str,
        input: &EntryInput,
    ) -> Result<CronEntryDto, CoreError> {
        actor.authorize(&OP_CRON_UPDATE, None)?;
        self.update_checked(actor, id, input).await
    }

    /// Partial update (REST `PATCH`): absent fields keep their stored value.
    pub async fn patch(
        &self,
        actor: &Actor,
        id: &str,
        p: &CronPatch,
    ) -> Result<CronEntryDto, CoreError> {
        actor.authorize(&OP_CRON_UPDATE, None)?;
        let cur = {
            let mut conn = self.reader().await?;
            repo::get(&mut conn, id)
                .await?
                .ok_or_else(|| CoreError::not_found("Cron Entry not found"))?
        };
        let input = EntryInput {
            name: p.name.clone().unwrap_or(cur.name),
            schedule: p.schedule.clone().unwrap_or(cur.schedule),
            run_as_user: p.run_as_user.clone().unwrap_or(cur.run_as_user),
            command: p.command.clone().unwrap_or(cur.command),
            overlap: p
                .overlap
                .or_else(|| cron::Overlap::parse(&cur.overlap))
                .unwrap_or(cron::Overlap::Allow),
            timeout_s: p.timeout_s.or(cur.timeout_s),
            enabled: p.enabled.unwrap_or(cur.enabled),
        };
        self.update_checked(actor, id, &input).await
    }

    async fn update_checked(
        &self,
        actor: &Actor,
        id: &str,
        input: &EntryInput,
    ) -> Result<CronEntryDto, CoreError> {
        let v = self.validate(input)?;
        let mut tx = self.ctx.db.begin_write().await?;
        if !repo::update(&mut tx, id, &fields(&v), self.ctx.now()).await? {
            return Err(CoreError::not_found("Cron Entry not found"));
        }
        self.apply(&mut tx, false).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cron.update", "cron_entry")
                .entity(id)
                .meta(audit_meta(&v)),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        self.entry_dto(id).await
    }

    pub async fn toggle(
        &self,
        actor: &Actor,
        id: &str,
        enabled: bool,
    ) -> Result<CronEntryDto, CoreError> {
        actor.authorize(&OP_CRON_TOGGLE, None)?;
        let mut tx = self.ctx.db.begin_write().await?;
        if !repo::set_enabled(&mut tx, id, enabled, self.ctx.now()).await? {
            return Err(CoreError::not_found("Cron Entry not found"));
        }
        self.apply(&mut tx, false).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cron.toggle", "cron_entry")
                .entity(id)
                .meta(json!({ "enabled": enabled })),
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        self.entry_dto(id).await
    }

    pub async fn delete(&self, actor: &Actor, id: &str) -> Result<(), CoreError> {
        actor.authorize(&OP_CRON_DELETE, None)?;
        let mut tx = self.ctx.db.begin_write().await?;
        let Some(row) = repo::get(&mut tx, id).await? else {
            return Err(CoreError::not_found("Cron Entry not found"));
        };
        repo::delete(&mut tx, id).await?;
        self.apply(&mut tx, false).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cron.delete", "cron_entry")
                .entity(id)
                .meta(json!({ "name": row.name, "schedule": row.schedule,
                    "run_as_user": row.run_as_user, "command": row.command })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    /// Overwrite the file with the DB state (drift "re-apply"). `replace_unsafe` lets the Owner
    /// replace a symlink or wrongly owned file (5.5).
    pub async fn reapply(&self, actor: &Actor, replace_unsafe: bool) -> Result<(), CoreError> {
        actor.authorize(&OP_CRON_REAPPLY, None)?;
        let mut tx = self.ctx.db.begin_write().await?;
        self.apply(&mut tx, replace_unsafe).await?;
        audit::write(
            &self.ctx,
            &mut tx,
            &AuditActor::from_actor(actor),
            &AuditEvent::new("cron.reapply", "cron_file")
                .meta(json!({ "replace_unsafe": replace_unsafe })),
        )
        .await?;
        tx.commit().await.map_err(db_err)
    }

    async fn entry_dto(&self, id: &str) -> Result<CronEntryDto, CoreError> {
        let mut conn = self.reader().await?;
        let r = repo::get(&mut conn, id)
            .await?
            .ok_or_else(|| CoreError::not_found("Cron Entry not found"))?;
        Ok(CronEntryDto {
            invalid_user: self.host.users.lookup(&r.run_as_user)?.is_none(),
            next_run: Schedule::parse(&r.schedule)
                .ok()
                .filter(|_| r.enabled)
                .and_then(|s| s.next_runs(self.ctx.now(), 1).first().copied()),
            last_run: None,
            id: r.id,
            name: r.name,
            schedule: r.schedule,
            run_as_user: r.run_as_user,
            command: r.command,
            overlap: r.overlap,
            timeout_s: r.timeout_s,
            enabled: r.enabled,
            project_id: r.project_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }

    /// `yanto cron-run <id>`: run the entry's command as its user and record the run (D26).
    /// The command is read from the DB, so the cron file never carries user text.
    pub async fn run_entry(&self, actor: &Actor, id: &str) -> Result<RunReport, CoreError> {
        actor.authorize(&OP_CRON_RUN, None)?;
        let entry = {
            let mut conn = self.reader().await?;
            repo::get(&mut conn, id)
                .await?
                .ok_or_else(|| CoreError::not_found("Cron Entry not found"))?
        };
        let run_id = new_id("crr");
        let started = self.ctx.now();
        {
            let mut tx = self.ctx.db.begin_write().await?;
            repo::run_insert(&mut tx, &run_id, id, started).await?;
            tx.commit().await.map_err(db_err)?;
        }

        let (status, exit_code, output) = self.execute(&entry).await;
        let finished = self.ctx.now();
        let tail = (!output.is_empty()).then(|| output_tail(&output));
        let mut tx = self.ctx.db.begin_write().await?;
        repo::run_finish(
            &mut tx,
            &run_id,
            status,
            exit_code,
            finished,
            tail.as_deref(),
        )
        .await?;
        repo::runs_prune(
            &mut tx,
            id,
            KEEP_RUNS,
            finished - KEEP_RUN_DAYS * super::DAY_MS,
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok(RunReport {
            run_id,
            status: status.to_owned(),
            exit_code,
            duration_ms: finished - started,
        })
    }

    /// Returns (`status`, exit code, output). Never fails: problems become a `failed` run.
    async fn execute(&self, entry: &repo::CronEntryRow) -> (&'static str, Option<i32>, Vec<u8>) {
        let failed = |msg: String| ("failed", None, msg.into_bytes());
        if !entry.enabled {
            return ("skipped", None, b"entry is disabled".to_vec());
        }
        let user = match self.host.users.lookup(&entry.run_as_user) {
            Ok(Some(u)) => u,
            Ok(None) => return failed(format!("unknown user {:?}", entry.run_as_user)),
            Err(e) => return failed(e.message),
        };
        if user.uid == 0 && user.name != "root" {
            return failed(format!(
                "refusing to run: {:?} resolves to uid 0 but is not root",
                user.name
            ));
        }
        let spec = ExecSpec {
            entry_id: entry.id.clone(),
            command: entry.command.clone(),
            user,
            timeout: entry
                .timeout_s
                .and_then(|s| u64::try_from(s).ok())
                .map(std::time::Duration::from_secs),
            skip_if_running: entry.overlap == "skip",
        };
        let exec = Arc::clone(&self.host.cron_exec);
        match tokio::task::spawn_blocking(move || exec.run(&spec)).await {
            Ok(Ok(ExecOutcome::Skipped)) => (
                "skipped",
                None,
                b"previous run is still in progress (overlap = skip)".to_vec(),
            ),
            Ok(Ok(ExecOutcome::Finished {
                timed_out: true,
                output,
                ..
            })) => ("timeout", None, output),
            Ok(Ok(ExecOutcome::Finished {
                exit_code: Some(0),
                output,
                ..
            })) => ("success", Some(0), output),
            Ok(Ok(ExecOutcome::Finished {
                exit_code, output, ..
            })) => ("failed", exit_code, output),
            Ok(Err(e)) => failed(e.message),
            Err(e) => failed(format!("run task failed: {e}")),
        }
    }
}
