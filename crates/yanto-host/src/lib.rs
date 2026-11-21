//! Real implementations of the core ports (docs/rewrite/08-conventions.md section 7): the Host's
//! cron files, `authorized_keys`, Unix accounts, process execution and backup helpers.
//!
//! Every file path is rooted in a constructor argument, so tests run against temp directories and
//! never touch `/etc` or a real home. Writers share [`fsutil`]: temp file in the same directory,
//! `fsync`, mode and owner, `rename`, refuse symlinks. Unix only (Linux target, builds on macOS).

mod authkeys;
mod backup;
mod cron;
mod cron_exec;
pub mod fsutil;
mod users;

pub use authkeys::{HostAuthorizedKeys, parse_sshd_t};
pub use backup::HostBackup;
pub use cron::HostCron;
pub use cron_exec::{HostCronExec, exec_as};
pub use users::{SystemUsers, parse_passwd};
