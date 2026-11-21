//! Read and atomically update a Unix user's `authorized_keys` (04 6.4).

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use yanto_core::authkeys::outside_block;
use yanto_core::ports::{
    AuthorizedKeysPort, HostUsers, KeysEditFn, KeysUpdated, SshdInfo, UnixUser,
};
use yanto_core::{CoreError, ErrorKind};

use crate::fsutil::{atomic_write, chown, io_err, sync_dir};

const TMP_NAME: &str = ".authorized_keys.yanto.tmp";
const BAK_NAME: &str = "authorized_keys.yanto.bak";

/// File bytes plus `(size, mtime secs, mtime nanos, sha256)`.
type Snapshot = (Vec<u8>, (u64, i64, i64, String));

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

    /// Create `~/.ssh` (`0700`, owned by the user) or check an existing one (6.4 step 2).
    fn ensure_ssh_dir(user: &UnixUser) -> Result<PathBuf, CoreError> {
        let dir = Self::ssh_dir(user);
        match std::fs::symlink_metadata(&dir) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
                Err(CoreError::conflict(format!(
                    "{} is not a plain directory; refusing to write there",
                    dir.display()
                )))
            }
            Ok(m) if m.mode() & 0o022 != 0 => Err(CoreError::conflict(format!(
                "{} is writable by group or others (sshd StrictModes would reject the keys); fix its mode first",
                dir.display()
            ))),
            Ok(_) => Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&dir).map_err(|e| io_err("cannot create ~/.ssh", e))?;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                    .map_err(|e| io_err("cannot set ~/.ssh mode", e))?;
                chown(&dir, user.uid, user.gid)?;
                Ok(dir)
            }
            Err(e) => Err(io_err("cannot stat ~/.ssh", e)),
        }
    }

    /// Snapshot of the file: bytes plus `(size, mtime, sha256)` for optimistic concurrency.
    fn snapshot(path: &Path) -> Result<Option<Snapshot>, CoreError> {
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => Err(CoreError::conflict(format!(
                "{} is a symbolic link; refusing to write through it",
                path.display()
            ))),
            Ok(m) if !m.is_file() => Err(CoreError::conflict(format!(
                "{} is not a regular file",
                path.display()
            ))),
            Ok(m) => {
                let bytes =
                    std::fs::read(path).map_err(|e| io_err("cannot read authorized_keys", e))?;
                let sha = yanto_core::crypto::hex(&Sha256::digest(&bytes));
                Ok(Some((bytes, (m.len(), m.mtime(), m.mtime_nsec(), sha))))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err("cannot stat authorized_keys", e)),
        }
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
        Self::ssh_dir(user).join("authorized_keys")
    }

    fn read(&self, user: &str) -> Result<Option<Vec<u8>>, CoreError> {
        let u = self.resolve(user)?;
        Ok(Self::snapshot(&self.file_path(&u))?.map(|(b, _)| b))
    }

    fn update(&self, user: &str, edit: &mut KeysEditFn<'_>) -> Result<KeysUpdated, CoreError> {
        let u = self.resolve(user)?;
        let dir = Self::ensure_ssh_dir(&u)?;
        let path = dir.join("authorized_keys");
        let tmp = dir.join(TMP_NAME);
        let bak = dir.join(BAK_NAME);
        let owner = Some((u.uid, u.gid));

        for attempt in 0..2 {
            let before = Self::snapshot(&path)?;
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
            // Stage the new content, then make sure nobody edited the file in the meantime.
            let _ = std::fs::remove_file(&tmp);
            std::fs::write(&tmp, &new.bytes)
                .map_err(|e| io_err("cannot stage authorized_keys", e))?;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| io_err("cannot set file mode", e))?;
            let again = Self::snapshot(&path)?;
            if again.as_ref().map(|(_, s)| s) != before.as_ref().map(|(_, s)| s) {
                let _ = std::fs::remove_file(&tmp);
                if attempt == 0 {
                    continue;
                }
                return Err(CoreError::conflict(
                    "authorized_keys changed while writing; reload and retry",
                ));
            }
            let _ = std::fs::remove_file(&tmp);

            let backup_path = if let Some((old, _)) = &before {
                atomic_write(
                    &dir.join(".authorized_keys.yanto.bak.tmp"),
                    &bak,
                    old,
                    0o600,
                    owner,
                )?;
                Some(bak.clone())
            } else {
                None
            };
            atomic_write(&tmp, &path, &new.bytes, 0o600, owner)?;

            // Verify; on any mismatch put the previous content back and fail loudly (6.4 step 7).
            let written =
                std::fs::read(&path).map_err(|e| io_err("cannot re-read authorized_keys", e))?;
            let outside_ok = outside_block(&written).is_ok_and(|o| o == new.expected_outside);
            if written != new.bytes || !outside_ok {
                if let Some((old, _)) = &before {
                    atomic_write(&tmp, &path, old, 0o600, owner)?;
                } else {
                    let _ = std::fs::remove_file(&path);
                    let _ = sync_dir(&dir);
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
