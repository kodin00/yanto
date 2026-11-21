//! Unix account lookups (`getpwnam`, `/etc/passwd`). Never reads `$HOME`.

use std::path::PathBuf;

use crate::error::CoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnixUser {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub shell: String,
}

pub trait HostUsers: Send + Sync + 'static {
    /// `getpwnam`: `None` when the account does not exist.
    fn lookup(&self, name: &str) -> Result<Option<UnixUser>, CoreError>;

    /// `root` plus accounts that can log in (real shell, not a system account), for dropdowns.
    fn login_users(&self) -> Result<Vec<UnixUser>, CoreError>;
}
