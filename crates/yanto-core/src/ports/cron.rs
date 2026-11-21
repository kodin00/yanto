//! Host side of Cron Entries: the managed `/etc/cron.d/yanto`, the read-only view of every other
//! cron source, and running an entry's command as its `run_as_user` (docs/rewrite/04 section 5).

use std::time::Duration;

use super::UnixUser;
use crate::error::CoreError;

/// State of the managed file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedFile {
    Missing,
    Present(String),
    /// A symlink, wrong owner or group/other-writable file: Yanto will not write through it (5.5).
    Unsafe(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CronSourceKind {
    Crontab,
    CronD,
    Periodic,
    UserSpool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", content = "text", rename_all = "snake_case")]
pub enum CronContent {
    Text(String),
    /// Longer than the 256 KiB cap; the text is the first part.
    Truncated(String),
    /// Not UTF-8, or not readable (carries the reason).
    Unreadable(String),
    /// Only the name is listed (`cron.hourly` and friends).
    NameOnly,
}

/// One read-only cron source (never edited by Yanto).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CronSource {
    pub path: String,
    pub kind: CronSourceKind,
    pub content: CronContent,
}

pub trait CronPort: Send + Sync + 'static {
    /// Absolute path of the running `yanto` binary, used in the generated lines.
    fn yanto_binary(&self) -> Result<String, CoreError>;

    fn read_managed(&self) -> Result<ManagedFile, CoreError>;

    /// Atomically replace the managed file (`0644 root:root`). Refuses an [`ManagedFile::Unsafe`]
    /// target unless `replace_unsafe`.
    fn write_managed(&self, content: &str, replace_unsafe: bool) -> Result<(), CoreError>;

    /// `/etc/crontab`, other `/etc/cron.d` files, `cron.{hourly,daily,weekly,monthly}` names and
    /// per-user spools. Read-only.
    fn system_sources(&self) -> Result<Vec<CronSource>, CoreError>;

    /// Whether the cron daemon is active; `None` when it cannot be determined.
    fn service_active(&self) -> Option<bool>;
}

/// What to run for one `yanto cron-run`.
#[derive(Debug, Clone)]
pub struct ExecSpec {
    pub entry_id: String,
    pub command: String,
    pub user: UnixUser,
    pub timeout: Option<Duration>,
    /// `overlap = skip`: do nothing when the previous run still holds the entry's lock.
    pub skip_if_running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecOutcome {
    /// Another run holds the lock and the entry says `overlap = skip`.
    Skipped,
    Finished {
        /// `None` when the process was killed by a signal.
        exit_code: Option<i32>,
        timed_out: bool,
        /// Combined stdout+stderr, at most the last 64 KiB.
        output: Vec<u8>,
    },
}

pub trait CronExec: Send + Sync + 'static {
    /// Run `/bin/sh -c <command>` as `spec.user` (blocking). `Err` means the command could not
    /// be started (unknown user, refused privilege drop, spawn failure).
    fn run(&self, spec: &ExecSpec) -> Result<ExecOutcome, CoreError>;
}
