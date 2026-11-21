//! Host side of Authorized Keys: read and atomically update a Unix user's `authorized_keys`
//! (docs/rewrite/04 section 6.4). The splice itself is pure ([`crate::authkeys`]); the port owns
//! resolution, permissions, optimistic concurrency, `.bak`, rename and verification.

use std::path::PathBuf;

use super::UnixUser;
use crate::error::CoreError;

/// New file content computed by the caller from the current bytes.
#[derive(Debug, Clone)]
pub struct KeysEdit {
    pub bytes: Vec<u8>,
    /// What [`crate::authkeys::outside_block`] of `bytes` must equal after the write.
    pub expected_outside: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysUpdated {
    /// `false` when the edit produced identical bytes (nothing was written).
    pub changed: bool,
    /// The `.bak` copy of the previous content, when a file existed.
    pub backup_path: Option<PathBuf>,
}

/// Informational `sshd -T` results (6.4 step 8). All fields are best effort.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SshdInfo {
    pub pubkey_authentication: Option<bool>,
    pub strict_modes: Option<bool>,
    pub authorized_keys_files: Vec<String>,
}

pub trait AuthorizedKeysPort: Send + Sync + 'static {
    /// The account behind `user`; errors when it does not exist or has a non-absolute home.
    fn resolve(&self, user: &str) -> Result<UnixUser, CoreError>;

    /// Absolute path of the file (for display).
    fn file_path(&self, user: &UnixUser) -> PathBuf;

    /// Current bytes; `None` when the file does not exist. Refuses a symlink.
    fn read(&self, user: &str) -> Result<Option<Vec<u8>>, CoreError>;

    /// Locked read-modify-write. `edit` gets the current bytes (empty when absent) and returns the
    /// new content, or `None` to leave the file alone; it may be called twice when the file changed
    /// under the writer (retry once, 6.4 step 5). The port verifies the result and restores the
    /// `.bak` on mismatch.
    fn update(
        &self,
        user: &str,
        edit: &mut dyn FnMut(&[u8]) -> Result<Option<KeysEdit>, CoreError>,
    ) -> Result<KeysUpdated, CoreError>;

    fn sshd_info(&self) -> Option<SshdInfo>;
}
