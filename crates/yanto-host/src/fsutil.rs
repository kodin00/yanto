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
            chown(tmp, uid, gid)?;
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
