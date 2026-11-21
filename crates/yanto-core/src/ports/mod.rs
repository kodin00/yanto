//! Port traits: every side effect the service layer needs from the outside
//! world (time, Docker, filesystem writes under `/etc`, ...). Real
//! implementations live in `yanto-host` (or here when trivial); fakes live in
//! [`crate::testing`]. See docs/rewrite/08-conventions.md "Ports and fakes".
//!
//! One file per concern, one `pub mod` line per file below.

mod authkeys;
mod backup;
mod clock;
mod cron;
mod hostmgmt;
mod users;

pub use authkeys::{AuthorizedKeysPort, KeysEdit, KeysUpdated, SshdInfo};
pub use backup::{
    BackupHost, LockGuard, ProjectBackupPort, ProjectBackupRequest, UnavailableProjectBackups,
};
pub use clock::{Clock, SystemClock};
pub use cron::{
    CronContent, CronExec, CronPort, CronSource, CronSourceKind, ExecOutcome, ExecSpec, ManagedFile,
};
pub use hostmgmt::{HostMgmtPorts, Unsupported};
pub use users::{HostUsers, UnixUser};
