//! Files Yanto keeps under `<data_dir>`: Project directories, `.env` files, and the
//! short-lived Deploy Key file used for a clone. All paths are derived from the slug and
//! containment-checked; nothing here touches anything outside the data dir.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::CoreError;

pub fn projects_root(data_dir: &Path) -> PathBuf {
    data_dir.join("projects")
}

/// `<data_dir>/projects/<slug>`. The slug is validated by the schema, but guard anyway.
pub fn project_dir(data_dir: &Path, slug: &str) -> Result<PathBuf, CoreError> {
    if slug.is_empty() || slug.contains('/') || slug.contains("..") || slug.starts_with('.') {
        return Err(CoreError::new(
            crate::ErrorKind::Internal,
            "unsafe project slug",
        ));
    }
    Ok(projects_root(data_dir).join(slug))
}

fn io_err(what: &'static str) -> impl FnOnce(std::io::Error) -> CoreError {
    move |e| CoreError::internal(what, e)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(io_err("cannot set file permissions"))
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> Result<(), CoreError> {
    Ok(())
}

/// Create a directory (and parents) with mode `0750` for the leaf.
pub fn ensure_dir(path: &Path, mode: u32) -> Result<(), CoreError> {
    std::fs::create_dir_all(path).map_err(io_err("cannot create directory"))?;
    set_mode(path, mode)
}

pub fn ensure_project_dir(data_dir: &Path, slug: &str) -> Result<PathBuf, CoreError> {
    let root = projects_root(data_dir);
    ensure_dir(&root, 0o750)?;
    let dir = project_dir(data_dir, slug)?;
    ensure_dir(&dir, 0o750)?;
    Ok(dir)
}

/// Write `content` atomically (temp file in the same directory, fsync, rename) with `mode`.
pub fn write_atomic(path: &Path, content: &str, mode: u32) -> Result<(), CoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::new(crate::ErrorKind::Internal, "path has no parent"))?;
    std::fs::create_dir_all(parent).map_err(io_err("cannot create directory"))?;
    if let Ok(meta) = std::fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        return Err(CoreError::new(
            crate::ErrorKind::Conflict,
            "refusing to write through a symlink",
        ));
    }
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        crate::crypto::random_hex::<4>()
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    let result = (|| -> std::io::Result<()> {
        let mut f = opts.open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::internal("cannot write file", e));
    }
    set_mode(path, mode)
}

/// A file relative to a Project directory, rejecting escapes.
pub fn resolve_in(dir: &Path, relative: &str) -> Result<PathBuf, CoreError> {
    if relative.starts_with('/') || relative.split('/').any(|c| c == "..") {
        return Err(CoreError::validation(
            "path",
            "path escapes the project directory",
        ));
    }
    Ok(dir.join(relative))
}

pub fn read_optional(path: &Path) -> Result<Option<String>, CoreError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CoreError::internal("cannot read file", e)),
    }
}

/// Remove a Project directory after asserting it is inside `<data_dir>/projects`.
pub fn remove_project_dir(data_dir: &Path, slug: &str) -> Result<(), CoreError> {
    let dir = project_dir(data_dir, slug)?;
    if !dir.exists() {
        return Ok(());
    }
    let root = std::fs::canonicalize(projects_root(data_dir))
        .map_err(io_err("cannot resolve projects dir"))?;
    let real = std::fs::canonicalize(&dir).map_err(io_err("cannot resolve project dir"))?;
    if real.parent() != Some(root.as_path()) {
        return Err(CoreError::new(
            crate::ErrorKind::Internal,
            "project directory is outside the projects root",
        ));
    }
    std::fs::remove_dir_all(&real).map_err(io_err("cannot remove project directory"))
}

/// A Deploy Key written to `<data_dir>/ssh/<slug>/id_ed25519` for the length of one
/// Deployment (dir `0700`, file `0600`). The file is removed on drop.
pub struct KeyFile {
    dir: PathBuf,
    path: PathBuf,
}

impl KeyFile {
    pub fn materialize(data_dir: &Path, slug: &str, private_key: &str) -> Result<Self, CoreError> {
        let dir = data_dir.join("ssh").join(slug);
        ensure_dir(&data_dir.join("ssh"), 0o700)?;
        ensure_dir(&dir, 0o700)?;
        let path = dir.join("id_ed25519");
        write_atomic(&path, private_key, 0o600)?;
        Ok(Self { dir, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for KeyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_sets_mode_and_refuses_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x/.env");
        write_atomic(&p, "A=1\n", 0o600).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "A=1\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let link = d.path().join("link");
            std::os::unix::fs::symlink(&p, &link).unwrap();
            assert!(write_atomic(&link, "B=2\n", 0o600).is_err());
        }
    }

    #[test]
    fn removal_is_contained_in_the_projects_root() {
        let d = tempfile::tempdir().unwrap();
        let dir = ensure_project_dir(d.path(), "app").unwrap();
        std::fs::write(dir.join("f"), "x").unwrap();
        remove_project_dir(d.path(), "app").unwrap();
        assert!(!dir.exists());
        assert!(project_dir(d.path(), "../etc").is_err());
        assert!(resolve_in(&dir, "../x").is_err());
    }

    #[test]
    fn key_file_is_private_and_removed_on_drop() {
        let d = tempfile::tempdir().unwrap();
        let path;
        {
            let k = KeyFile::materialize(d.path(), "app", "KEY").unwrap();
            path = k.path().to_owned();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            assert!(path.exists());
        }
        assert!(!path.exists());
    }
}
