//! [`TraefikFilesPort`]: the one dynamic file Yanto owns (`<dynamic_dir>/yanto.yml`).
//!
//! The temp file (`.yanto.yml.tmp`) and the backup (`yanto.yml.bak`) live *inside* the dynamic
//! dir: the systemd unit (`ProtectSystem=strict`) only makes that dir writable, and a rename
//! from its parent would cross the `ReadWritePaths` bind mount (EXDEV). Traefik's file provider
//! only loads `.yml`/`.yaml`/`.toml` files, so neither is ever parsed; then `fsync`, `rename`,
//! `fsync` of the directory.

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
        // Temp file, backup and rename target all live in the dynamic dir.
        let writable = exists && access(&self.dir, AccessFlags::W_OK).is_ok();
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
        let dir = &self.dir;
        if let Some(old) = self.read_dynamic()? {
            atomic_write(
                &dir.join(".yanto.yml.bak.tmp"),
                &dir.join("yanto.yml.bak"),
                old.as_bytes(),
                0o600,
                self.owner,
            )?;
        }
        atomic_write(
            &dir.join(".yanto.yml.tmp"),
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
        let mut in_dir: Vec<_> = std::fs::read_dir(t.dynamic_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        in_dir.sort();
        assert_eq!(
            in_dir,
            vec!["yanto.yml", "yanto.yml.bak"],
            "only the managed file and its backup (not a .yml, so Traefik skips it)"
        );
        let bak = std::fs::read_to_string(t.dynamic_dir().join("yanto.yml.bak")).unwrap();
        assert_eq!(bak, "first\n");
        // Nothing is staged outside the dynamic dir.
        let outside: Vec<_> = std::fs::read_dir(root.path().join("traefik"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(outside, vec!["dynamic"]);
    }

    #[test]
    fn only_the_dynamic_dir_needs_to_be_writable() {
        use std::os::unix::fs::PermissionsExt;
        let (root, t) = setup();
        t.write_dynamic("first\n").unwrap();
        // ProtectSystem=strict: the parent is read-only, only the dynamic dir is a RW path.
        let parent = root.path().join("traefik");
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();
        let status = t.dir_status();
        let r = t.write_dynamic("second\n");
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        r.unwrap();
        assert!(status.writable);
        assert_eq!(t.read_dynamic().unwrap().as_deref(), Some("second\n"));
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
