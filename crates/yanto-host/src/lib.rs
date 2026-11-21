//! Real implementations of the core ports (docs/rewrite/08-conventions.md section 7): Docker, host
//! stats, and the Host's cron files, `authorized_keys`, Unix accounts, process execution and
//! backup helpers.
//!
//! The M5 file-based adapters root every path in a constructor argument, so tests run against temp
//! directories and never touch `/etc` or a real home. Writers share [`fsutil`]: temp file in the
//! same directory, `fsync`, mode and owner, `rename`, refuse symlinks. Unix only (Linux target,
//! builds on macOS).

mod authkeys;
mod backup;
mod cron;
mod cron_exec;
mod docker;
pub mod fsutil;
mod stats;
mod users;

pub use authkeys::{HostAuthorizedKeys, parse_sshd_t};
pub use backup::HostBackup;
pub use cron::HostCron;
pub use cron_exec::{HostCronExec, exec_as};
pub use docker::BollardDocker;
pub use stats::SysinfoStats;
pub use users::{SystemUsers, parse_passwd};
