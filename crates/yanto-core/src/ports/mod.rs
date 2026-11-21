//! Port traits: every side effect the service layer needs from the outside
//! world (time, Docker, filesystem writes under `/etc`, ...). Real
//! implementations live in `yanto-host` (or here when trivial); fakes live in
//! [`crate::testing`]. See docs/rewrite/08-conventions.md "Ports and fakes".
//!
//! One file per concern, one `pub mod` line per file below.

mod authkeys;
mod backup;
mod clock;
mod command;
mod cron;
mod docker;
mod host_probe;
mod host_stats;
mod hostmgmt;
mod registry_auth;
mod users;

pub use authkeys::{AuthorizedKeysPort, KeysEdit, KeysEditFn, KeysUpdated, SshdInfo};
pub use backup::{
    BackupHost, DbTarget, LockGuard, ProjectBackupPort, ProjectBackupRequest, ProjectRestoreRequest,
    RestoreMode, UnavailableProjectBackups,
};
pub use clock::{Clock, SystemClock};
pub use command::{
    CancellationToken, CommandLogStream, CommandOutput, CommandPort, CommandSpec, EXIT_CANCELLED,
    EXIT_TIMED_OUT, LogSink, NullSink,
};
pub use cron::{
    CronContent, CronExec, CronPort, CronSource, CronSourceKind, ExecOutcome, ExecSpec, ManagedFile,
};
pub use docker::*;
pub use host_probe::{HostProbePort, Listener};
pub use host_stats::*;
pub use hostmgmt::{HostMgmtPorts, Unsupported};
pub use registry_auth::{AuthEntry, RegistryAuthPort};
pub use users::{HostUsers, UnixUser};
