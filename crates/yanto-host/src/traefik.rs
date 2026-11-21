//! [`TraefikFilesPort`]: the one dynamic file Yanto owns (`<dynamic_dir>/yanto.yml`).
//!
//! The temp file lives in the *parent* of the dynamic dir so Traefik's directory watcher never
//! sees a partial or stray file (ASSUMPTION: same filesystem, 04 9.3); then `fsync`, `rename`,
//! `fsync` of the directory. The previous content is kept as `<dir>/../yanto.yml.bak`.

use std::path::{Path, PathBuf};

use nix::unistd::{AccessFlags, access};
use yanto_core::CoreError;
use yanto_core::ports::{DirStatus, TraefikFilesPort};

use crate::fsutil::{atomic_write, io_err};

pub struct HostTraefik {
    dir: PathBuf,
    owner: Option<(u32, u32)>,
}

impl HostTraefik {
    pub fn new(dynamic_dir: &Path, owner: Option<(u32, u32)>) -> Self {
        Self {
            dir: dynamic_dir.to_path_buf(),
            owner,
        }
    }

    fn parent(&self) -> PathBuf {
        self.dir
            .parent()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf)
    }
}

fn refuse_symlink(p: &Path) -> Result<(), CoreError> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => Err(CoreError::new(
            yanto_core::ErrorKind::Conflict,
            format!("{} is a symlink; refusing to write through it", p.display()),
        )),
        _ => Ok(()),
    }
}

impl TraefikFilesPort for HostTraefik {
    fn dynamic_dir(&self) -> PathBuf {
        self.dir.clone()
    }

    fn dynamic_file(&self) -> PathBuf {
        self.dir.join("yanto.yml")
    }

    fn dir_status(&self) -> DirStatus {
        let exists = self.dir.is_dir();
        // Writing needs the dynamic dir (rename target) and its parent (temp file).
        let writable = exists
            && access(&self.dir, AccessFlags::W_OK).is_ok()
            && access(&self.parent(), AccessFlags::W_OK).is_ok();
        DirStatus { exists, writable }
    }

    fn read_dynamic(&self) -> Result<Option<String>, CoreError> {
        match std::fs::read_to_string(self.dynamic_file()) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err("cannot read yanto.yml", e)),
        }
    }

    fn write_dynamic(&self, content: &str) -> Result<(), CoreError> {
        let dest = self.dynamic_file();
        refuse_symlink(&dest)?;
        let parent = self.parent();
        if let Some(old) = self.read_dynamic()? {
            atomic_write(
                &parent.join(".yanto.yml.bak.tmp"),
                &parent.join("yanto.yml.bak"),
                old.as_bytes(),
                0o600,
                self.owner,
            )?;
        }
        atomic_write(
            &parent.join(".yanto.yml.tmp"),
            &dest,
            content.as_bytes(),
            0o644,
            self.owner,
        )
    }

    fn read_static(&self, path: &Path) -> Result<Option<String>, CoreError> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err("cannot read the Traefik static config", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, HostTraefik) {
        let root = tempfile::tempdir().unwrap();
        let dynamic = root.path().join("traefik/dynamic");
        std::fs::create_dir_all(&dynamic).unwrap();
        (root, HostTraefik::new(&dynamic, None))
    }

    #[test]
    fn write_is_atomic_leaves_no_stray_files_and_keeps_a_backup() {
        let (root, t) = setup();
        assert_eq!(t.read_dynamic().unwrap(), None);
        t.write_dynamic("first\n").unwrap();
        t.write_dynamic("second\n").unwrap();
        assert_eq!(t.read_dynamic().unwrap().as_deref(), Some("second\n"));
        let in_dir: Vec<_> = std::fs::read_dir(t.dynamic_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            in_dir,
            vec!["yanto.yml"],
            "nothing but the managed file in the watched dir"
        );
        let bak = std::fs::read_to_string(root.path().join("traefik/yanto.yml.bak")).unwrap();
        assert_eq!(bak, "first\n");
        let leftovers: Vec<_> = std::fs::read_dir(root.path().join("traefik"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn symlink_is_refused_and_status_reports_the_dir() {
        let (root, t) = setup();
        let target = root.path().join("elsewhere.yml");
        std::fs::write(&target, "x").unwrap();
        std::os::unix::fs::symlink(&target, t.dynamic_file()).unwrap();
        assert!(t.write_dynamic("y").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "x");
        assert_eq!(
            t.dir_status(),
            DirStatus {
                exists: true,
                writable: true
            }
        );
        let missing = HostTraefik::new(&root.path().join("nope/dynamic"), None);
        assert!(!missing.dir_status().exists);
        assert_eq!(
            missing.read_static(&root.path().join("none.yml")).unwrap(),
            None
        );
    }
}
