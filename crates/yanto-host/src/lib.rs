//! Real implementations of the core ports (docs/rewrite/08-conventions.md section 7): Docker, host
//! stats, the Host's cron files, `authorized_keys`, Unix accounts, process execution, backup
//! helpers, and the M2 deploy adapters (command runner, `/proc` probe, registry auth).
//!
//! The M5 file-based adapters root every path in a constructor argument, so tests run against temp
//! directories and never touch `/etc` or a real home. Writers share [`fsutil`]: temp file in the
//! same directory, `fsync`, mode and owner, `rename`, refuse symlinks. Unix only (Linux target,
//! builds on macOS). Host-integration tests live next to the code or in `tests/`; anything needing
//! a real Docker daemon is `#[ignore]`.

mod authkeys;
mod backup;
pub mod commands;
mod cron;
mod cron_exec;
mod docker;
pub mod fsutil;
pub mod probe;
pub mod registry_auth;
mod stats;
mod users;

pub use authkeys::{HostAuthorizedKeys, parse_sshd_t};
pub use backup::HostBackup;
pub use commands::TokioCommands;
pub use cron::HostCron;
pub use cron_exec::{HostCronExec, exec_as};
pub use docker::BollardDocker;
pub use probe::ProcProbe;
pub use registry_auth::RunDirRegistryAuth;
pub use stats::SysinfoStats;
pub use users::{SystemUsers, parse_passwd};
