//! Read and atomically update a Unix user's `authorized_keys` (04 6.4).

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use yanto_core::authkeys::outside_block;
use yanto_core::ports::{
    AuthorizedKeysPort, HostUsers, KeysEditFn, KeysUpdated, SshdInfo, UnixUser,
};
use yanto_core::{CoreError, ErrorKind};

use crate::fsutil::{DirHandle, io_err};

const FILE_NAME: &str = "authorized_keys";
const TMP_NAME: &str = ".authorized_keys.yanto.tmp";
const BAK_NAME: &str = "authorized_keys.yanto.bak";
const BAK_TMP_NAME: &str = ".authorized_keys.yanto.bak.tmp";

/// File bytes plus `(size, mtime secs, mtime nanos, sha256)`.
type Snapshot = (Vec<u8>, (u64, i64, i64, String));

/// Yanto runs as root but `~/.ssh` belongs to the (untrusted) target user, who can rename and
/// symlink entries in it at any time. Every step therefore goes through one [`DirHandle`]
/// opened with `O_NOFOLLOW`: nothing is re-resolved by path after the checks.
#[derive(Clone)]
pub struct HostAuthorizedKeys {
    users: Arc<dyn HostUsers>,
}

impl HostAuthorizedKeys {
    pub fn new(users: Arc<dyn HostUsers>) -> Self {
        Self { users }
    }

    fn ssh_dir(user: &UnixUser) -> PathBuf {
        user.home.join(".ssh")
    }

    /// Open `~/.ssh` (creating it `0700`, owned by the user) and check it (6.4 step 2).
    fn ensure_ssh_dir(user: &UnixUser) -> Result<DirHandle, CoreError> {
        let path = Self::ssh_dir(user);
        let dir = match DirHandle::open(&path)? {
            Some(d) => d,
            None => {
                match std::fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(io_err("cannot create ~/.ssh", e)),
                }
                let d = DirHandle::open(&path)?
                    .ok_or_else(|| CoreError::conflict("~/.ssh disappeared while creating it"))?;
                d.set_mode_owner(0o700, Some((user.uid, user.gid)))?;
                d
            }
        };
        if dir.metadata()?.mode() & 0o022 != 0 {
            return Err(CoreError::conflict(format!(
                "{} is writable by group or others (sshd StrictModes would reject the keys); fix its mode first",
                path.display()
            )));
        }
        Ok(dir)
    }

    /// Snapshot of the file: bytes plus `(size, mtime, sha256)` for optimistic concurrency.
    fn snapshot(dir: &DirHandle) -> Result<Option<Snapshot>, CoreError> {
        Ok(dir.read_file(FILE_NAME)?.map(|(bytes, m)| {
            let sha = yanto_core::crypto::hex(&Sha256::digest(&bytes));
            (bytes, (m.len(), m.mtime(), m.mtime_nsec(), sha))
        }))
    }
}

impl AuthorizedKeysPort for HostAuthorizedKeys {
    fn resolve(&self, user: &str) -> Result<UnixUser, CoreError> {
        let u = self.users.lookup(user)?.ok_or_else(|| {
            CoreError::validation(
                "user",
                format!("Unix user {user:?} does not exist on this Host"),
            )
        })?;
        if !u.home.is_absolute() {
            return Err(CoreError::validation(
                "user",
                format!("Unix user {user:?} has no absolute home directory"),
            ));
        }
        Ok(u)
    }

    fn file_path(&self, user: &UnixUser) -> PathBuf {
        Self::ssh_dir(user).join(FILE_NAME)
    }

    fn read(&self, user: &str) -> Result<Option<Vec<u8>>, CoreError> {
        let u = self.resolve(user)?;
        let Some(dir) = DirHandle::open(&Self::ssh_dir(&u))? else {
            return Ok(None);
        };
        Ok(Self::snapshot(&dir)?.map(|(b, _)| b))
    }

    fn update(&self, user: &str, edit: &mut KeysEditFn<'_>) -> Result<KeysUpdated, CoreError> {
        let u = self.resolve(user)?;
        let dir = Self::ensure_ssh_dir(&u)?;
        let owner = Some((u.uid, u.gid));

        for attempt in 0..2 {
            let before = Self::snapshot(&dir)?;
            let current = before
                .as_ref()
                .map(|(b, _)| b.as_slice())
                .unwrap_or_default();
            let Some(new) = edit(current)? else {
                return Ok(KeysUpdated {
                    changed: false,
                    backup_path: None,
                });
            };
            if before.as_ref().is_some_and(|(b, _)| *b == new.bytes) {
                return Ok(KeysUpdated {
                    changed: false,
                    backup_path: None,
                });
            }
            // Make sure nobody edited the file while the new content was computed.
            let again = Self::snapshot(&dir)?;
            if again.as_ref().map(|(_, s)| s) != before.as_ref().map(|(_, s)| s) {
                if attempt == 0 {
                    continue;
                }
                return Err(CoreError::conflict(
                    "authorized_keys changed while writing; reload and retry",
                ));
            }

            let backup_path = if let Some((old, _)) = &before {
                dir.write_atomic(BAK_TMP_NAME, BAK_NAME, old, 0o600, owner)?;
                Some(dir.path().join(BAK_NAME))
            } else {
                None
            };
            dir.write_atomic(TMP_NAME, FILE_NAME, &new.bytes, 0o600, owner)?;

            // Verify; on any mismatch put the previous content back and fail loudly (6.4 step 7).
            let written = Self::snapshot(&dir)?.map(|(b, _)| b).unwrap_or_default();
            let outside_ok = outside_block(&written).is_ok_and(|o| o == new.expected_outside);
            if written != new.bytes || !outside_ok {
                if let Some((old, _)) = &before {
                    dir.write_atomic(TMP_NAME, FILE_NAME, old, 0o600, owner)?;
                } else {
                    dir.remove(FILE_NAME)?;
                }
                return Err(CoreError::new(
                    ErrorKind::Internal,
                    "authorized_keys verification failed; the previous content was restored",
                ));
            }
            return Ok(KeysUpdated {
                changed: true,
                backup_path,
            });
        }
        Err(CoreError::conflict(
            "authorized_keys changed while writing; reload and retry",
        ))
    }

    fn sshd_info(&self) -> Option<SshdInfo> {
        let out = Command::new("sshd").arg("-T").output().ok()?;
        out.status
            .success()
            .then(|| parse_sshd_t(&String::from_utf8_lossy(&out.stdout)))
    }
}

/// Extract the three settings Yanto cares about from `sshd -T` output.
pub fn parse_sshd_t(text: &str) -> SshdInfo {
    let mut info = SshdInfo::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(k), rest) = (it.next(), it.collect::<Vec<_>>()) else {
            continue;
        };
        let yes = || rest.first().map(|v| v.eq_ignore_ascii_case("yes"));
        match k.to_ascii_lowercase().as_str() {
            "pubkeyauthentication" => info.pubkey_authentication = yes(),
            "strictmodes" => info.strict_modes = yes(),
            "authorizedkeysfile" => {
                info.authorized_keys_files = rest.iter().map(|s| (*s).to_owned()).collect()
            }
            _ => {}
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sshd_output_is_parsed() {
        let i = parse_sshd_t(
            "port 22\npubkeyauthentication yes\nstrictmodes no\nauthorizedkeysfile .ssh/authorized_keys .ssh/authorized_keys2\n",
        );
        assert_eq!(i.pubkey_authentication, Some(true));
        assert_eq!(i.strict_modes, Some(false));
        assert_eq!(i.authorized_keys_files.len(), 2);
        assert_eq!(parse_sshd_t(""), SshdInfo::default());
    }
}
