//! File helpers shared by every writer: atomic replace, private creation, locks.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use nix::fcntl::{Flock, FlockArg};
use nix::unistd::{Gid, Uid};
use yanto_core::CoreError;

pub fn io_err(what: &str, e: std::io::Error) -> CoreError {
    let kind = match e.kind() {
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => {
            yanto_core::ErrorKind::Unavailable
        }
        _ => yanto_core::ErrorKind::Internal,
    };
    CoreError::new(kind, format!("{what}: {e}")).with_source(e)
}

pub fn chown(path: &Path, uid: u32, gid: u32) -> Result<(), CoreError> {
    nix::unistd::chown(path, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)))
        .map_err(|e| io_err("chown failed", std::io::Error::from(e)))
}

/// Write `data` to `tmp` (replacing a stale one), set `mode` and owner, `fsync`, then `rename` over
/// `dest` and `fsync` the directory. `tmp` must be on the same filesystem as `dest`. A symlink at
/// `dest` is replaced, not followed (rename replaces the link itself).
pub fn atomic_write(
    tmp: &Path,
    dest: &Path,
    data: &[u8],
    mode: u32,
    owner: Option<(u32, u32)>,
) -> Result<(), CoreError> {
    let _ = std::fs::remove_file(tmp);
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(tmp)
        .map_err(|e| io_err("cannot create the temporary file", e))?;
    let res = (|| {
        f.write_all(data)
            .map_err(|e| io_err("cannot write the temporary file", e))?;
        // The umask may have narrowed the creation mode.
        f.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|e| io_err("cannot set file mode", e))?;
        if let Some((uid, gid)) = owner {
            // On the open handle: a path-based chown would follow a symlink swapped in at `tmp`.
            std::os::unix::fs::fchown(&f, Some(uid), Some(gid))
                .map_err(|e| io_err("chown failed", e))?;
        }
        f.sync_all()
            .map_err(|e| io_err("cannot sync the file", e))?;
        std::fs::rename(tmp, dest).map_err(|e| io_err("cannot replace the file", e))
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(tmp);
        return res;
    }
    if let Some(dir) = dest.parent() {
        sync_dir(dir)?;
    }
    Ok(())
}

pub fn sync_dir(dir: &Path) -> Result<(), CoreError> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| io_err("cannot sync the directory", e))
}

/// Non-blocking exclusive `flock`. `Ok(None)` when another holder has it. The lock lives as long
/// as the returned value.
pub fn try_flock(path: &Path) -> Result<Option<Flock<File>>, CoreError> {
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| io_err("cannot open the lock file", e))?;
    match Flock::lock(f, FlockArg::LockExclusiveNonblock) {
        Ok(l) => Ok(Some(l)),
        Err((_, nix::errno::Errno::EWOULDBLOCK)) => Ok(None),
        Err((_, e)) => Err(io_err("cannot take the lock", std::io::Error::from(e))),
    }
}

/// Owner uid and mode bits of a path without following symlinks.
pub fn owner_and_mode(path: &Path) -> std::io::Result<(u32, u32)> {
    let m = std::fs::symlink_metadata(path)?;
    Ok((m.uid(), m.mode() & 0o7777))
}

/// Create `dir` (and parents) when missing; the leaf is made `0700`.
pub fn ensure_private_dir(dir: &Path) -> Result<(), CoreError> {
    let existed = dir.is_dir();
    std::fs::create_dir_all(dir).map_err(|e| io_err("cannot create the directory", e))?;
    if !existed {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| io_err("cannot restrict the directory", e))?;
    }
    Ok(())
}

/// A directory opened once without following symlinks. Every later step is relative to the open
/// handle (`openat`/`renameat`/`unlinkat`, `O_NOFOLLOW`, `fchown`/`fchmod`), so whoever owns the
/// directory (e.g. a user's `~/.ssh`) cannot redirect a root read or write by swapping a path
/// component for a symlink between a check and the use.
#[derive(Debug)]
pub struct DirHandle {
    dir: File,
    path: std::path::PathBuf,
}

fn not_plain(what: &Path, why: &str) -> CoreError {
    CoreError::conflict(format!("{} {why}; refusing to use it", what.display()))
}

impl DirHandle {
    /// `Ok(None)` when `path` does not exist; `Conflict` when it is a symlink or not a directory.
    pub fn open(path: &Path) -> Result<Option<Self>, CoreError> {
        match OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .open(path)
        {
            Ok(dir) => Ok(Some(Self {
                dir,
                path: path.to_owned(),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(nix::libc::ELOOP | nix::libc::ENOTDIR)
                ) =>
            {
                Err(not_plain(path, "is not a plain directory"))
            }
            Err(e) => Err(io_err("cannot open the directory", e)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn metadata(&self) -> Result<std::fs::Metadata, CoreError> {
        self.dir
            .metadata()
            .map_err(|e| io_err("cannot stat the directory", e))
    }

    /// `fchmod` + `fchown` the directory itself.
    pub fn set_mode_owner(&self, mode: u32, owner: Option<(u32, u32)>) -> Result<(), CoreError> {
        self.dir
            .set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|e| io_err("cannot set the directory mode", e))?;
        if let Some((uid, gid)) = owner {
            std::os::unix::fs::fchown(&self.dir, Some(uid), Some(gid))
                .map_err(|e| io_err("chown failed", e))?;
        }
        Ok(())
    }

    /// Read a regular file in this directory. `Ok(None)` when missing; `Conflict` for a symlink
    /// or anything that is not a regular file (opened non-blocking so a FIFO cannot hang us).
    pub fn read_file(&self, name: &str) -> Result<Option<(Vec<u8>, std::fs::Metadata)>, CoreError> {
        use nix::fcntl::{OFlag, openat};
        use std::io::Read;
        let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK;
        let fd = match openat(&self.dir, name, flags, nix::sys::stat::Mode::empty()) {
            Ok(fd) => fd,
            Err(nix::errno::Errno::ENOENT) => return Ok(None),
            Err(nix::errno::Errno::ELOOP) => {
                return Err(not_plain(&self.path.join(name), "is a symbolic link"));
            }
            Err(e) => return Err(io_err("cannot open the file", std::io::Error::from(e))),
        };
        let mut f = File::from(fd);
        let meta = f
            .metadata()
            .map_err(|e| io_err("cannot stat the file", e))?;
        if !meta.is_file() {
            return Err(not_plain(&self.path.join(name), "is not a regular file"));
        }
        let mut buf = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
        f.read_to_end(&mut buf)
            .map_err(|e| io_err("cannot read the file", e))?;
        Ok(Some((buf, meta)))
    }

    /// [`atomic_write`] relative to this directory: `tmp` is created `O_EXCL|O_NOFOLLOW`, mode and
    /// owner are set on the handle, then `renameat` over `name` and `fsync` the directory.
    pub fn write_atomic(
        &self,
        tmp: &str,
        name: &str,
        data: &[u8],
        mode: u32,
        owner: Option<(u32, u32)>,
    ) -> Result<(), CoreError> {
        use nix::fcntl::{OFlag, openat, renameat};
        use nix::sys::stat::Mode;
        let _ = self.remove(tmp);
        let flags =
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
        let fd = openat(&self.dir, tmp, flags, Mode::S_IRUSR | Mode::S_IWUSR)
            .map_err(|e| io_err("cannot create the temporary file", std::io::Error::from(e)))?;
        let mut f = File::from(fd);
        let res = (|| {
            f.write_all(data)
                .map_err(|e| io_err("cannot write the temporary file", e))?;
            f.set_permissions(std::fs::Permissions::from_mode(mode))
                .map_err(|e| io_err("cannot set file mode", e))?;
            if let Some((uid, gid)) = owner {
                std::os::unix::fs::fchown(&f, Some(uid), Some(gid))
                    .map_err(|e| io_err("chown failed", e))?;
            }
            f.sync_all()
                .map_err(|e| io_err("cannot sync the file", e))?;
            renameat(&self.dir, tmp, &self.dir, name)
                .map_err(|e| io_err("cannot replace the file", std::io::Error::from(e)))
        })();
        if res.is_err() {
            let _ = self.remove(tmp);
            return res;
        }
        self.dir
            .sync_all()
            .map_err(|e| io_err("cannot sync the directory", e))
    }

    /// `unlinkat` a non-directory entry; a missing one is fine.
    pub fn remove(&self, name: &str) -> Result<(), CoreError> {
        match nix::unistd::unlinkat(&self.dir, name, nix::unistd::UnlinkatFlags::NoRemoveDir) {
            Ok(()) | Err(nix::errno::Errno::ENOENT) => Ok(()),
            Err(e) => Err(io_err("cannot remove the file", std::io::Error::from(e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn dir_handle_never_follows_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let outside = d.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("victim"), "precious").unwrap();
        let ssh = d.path().join("ssh");
        std::fs::create_dir(&ssh).unwrap();

        // The directory itself as a symlink is refused.
        symlink(&outside, d.path().join("link")).unwrap();
        assert!(DirHandle::open(&d.path().join("link")).is_err());
        assert!(
            DirHandle::open(&d.path().join("missing"))
                .unwrap()
                .is_none()
        );

        let h = DirHandle::open(&ssh).unwrap().unwrap();
        // A planted temp-file symlink is replaced, never written through.
        symlink(outside.join("victim"), ssh.join(".tmp")).unwrap();
        h.write_atomic(".tmp", "keys", b"new", 0o600, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(outside.join("victim")).unwrap(),
            "precious"
        );
        assert_eq!(std::fs::read_to_string(ssh.join("keys")).unwrap(), "new");
        assert_eq!(
            std::fs::metadata(ssh.join("keys")).unwrap().mode() & 0o777,
            0o600
        );
        // A symlinked file is not read.
        symlink(outside.join("victim"), ssh.join("peek")).unwrap();
        assert!(h.read_file("peek").is_err());
        assert!(h.read_file("nope").unwrap().is_none());

        // Swapping the directory path after opening does not redirect writes.
        std::fs::rename(&ssh, d.path().join("moved")).unwrap();
        symlink(&outside, &ssh).unwrap();
        h.write_atomic(".tmp", "keys2", b"x", 0o600, None).unwrap();
        assert!(!outside.join("keys2").exists());
        assert!(d.path().join("moved/keys2").exists());
    }
}
