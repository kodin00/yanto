//! The managed `/etc/cron.d/yanto` and the read-only view of every other cron source.

use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use yanto_core::cron::{CRON_FILE_NAME, CRON_TEMP_NAME};
use yanto_core::ports::{CronContent, CronPort, CronSource, CronSourceKind, ManagedFile};
use yanto_core::{CoreError, ErrorKind};

use crate::fsutil::{atomic_write, io_err, owner_and_mode};

/// Cap per displayed file (5.4).
const MAX_VIEW_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone)]
pub struct HostCron {
    etc_dir: PathBuf,
    spool_dir: PathBuf,
    /// Owner the managed file must have and gets (`(0, 0)` in production).
    owner: (u32, u32),
    binary: Option<PathBuf>,
    check_service: bool,
}

impl HostCron {
    /// `etc_dir` is `/etc` and `spool_dir` is `/var/spool/cron` in production; tests pass temp dirs
    /// and the current user's ids as `owner`.
    pub fn new(
        etc_dir: impl Into<PathBuf>,
        spool_dir: impl Into<PathBuf>,
        owner: (u32, u32),
    ) -> Self {
        Self {
            etc_dir: etc_dir.into(),
            spool_dir: spool_dir.into(),
            owner,
            binary: None,
            check_service: true,
        }
    }

    /// Use this path instead of `current_exe` in generated lines.
    pub fn with_binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.binary = Some(path.into());
        self
    }

    /// Skip `systemctl` (tests).
    pub fn without_service_check(mut self) -> Self {
        self.check_service = false;
        self
    }

    fn cron_d(&self) -> PathBuf {
        self.etc_dir.join("cron.d")
    }

    fn managed_path(&self) -> PathBuf {
        self.cron_d().join(CRON_FILE_NAME)
    }
}

fn describe(path: &Path, kind: CronSourceKind) -> CronSource {
    let content = match std::fs::symlink_metadata(path) {
        Err(e) => CronContent::Unreadable(e.to_string()),
        Ok(m) if m.file_type().is_symlink() => CronContent::Unreadable("symbolic link".into()),
        Ok(m) if !m.is_file() => CronContent::Unreadable("not a regular file".into()),
        Ok(m) => match std::fs::read(path) {
            Err(e) => CronContent::Unreadable(e.to_string()),
            Ok(bytes) => {
                let truncated = m.len() > MAX_VIEW_BYTES;
                let slice = &bytes[..bytes.len().min(MAX_VIEW_BYTES as usize)];
                match std::str::from_utf8(slice) {
                    Ok(t) if truncated => CronContent::Truncated(t.to_owned()),
                    Ok(t) => CronContent::Text(t.to_owned()),
                    // A cut inside a multi-byte char is still text; real binary is not.
                    Err(e) if truncated && e.error_len().is_none() => CronContent::Truncated(
                        String::from_utf8_lossy(&slice[..e.valid_up_to()]).into_owned(),
                    ),
                    Err(_) => CronContent::Unreadable("binary or not UTF-8".into()),
                }
            }
        },
    };
    CronSource {
        path: path.display().to_string(),
        kind,
        content,
    }
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

impl CronPort for HostCron {
    fn yanto_binary(&self) -> Result<String, CoreError> {
        let p = match &self.binary {
            Some(p) => p.clone(),
            None => {
                std::env::current_exe().map_err(|e| io_err("cannot locate the yanto binary", e))?
            }
        };
        Ok(p.display().to_string())
    }

    fn read_managed(&self) -> Result<ManagedFile, CoreError> {
        let path = self.managed_path();
        let m = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ManagedFile::Missing),
            Err(e) => return Err(io_err("cannot stat the cron file", e)),
        };
        if m.file_type().is_symlink() {
            return Ok(ManagedFile::Unsafe("it is a symbolic link".into()));
        }
        if !m.is_file() {
            return Ok(ManagedFile::Unsafe("it is not a regular file".into()));
        }
        let (uid, mode) =
            owner_and_mode(&path).map_err(|e| io_err("cannot stat the cron file", e))?;
        if uid != self.owner.0 {
            return Ok(ManagedFile::Unsafe(format!(
                "it is owned by uid {uid}, not {}",
                self.owner.0
            )));
        }
        if mode & 0o022 != 0 {
            return Ok(ManagedFile::Unsafe(format!(
                "its mode {mode:o} is writable by group or others (cron would ignore it)"
            )));
        }
        let bytes = std::fs::read(&path).map_err(|e| io_err("cannot read the cron file", e))?;
        Ok(ManagedFile::Present(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))
    }

    fn write_managed(&self, content: &str, replace_unsafe: bool) -> Result<(), CoreError> {
        if !content.ends_with('\n') {
            return Err(CoreError::new(
                ErrorKind::Internal,
                "cron file must end with a newline",
            ));
        }
        if !self.cron_d().is_dir() {
            return Err(CoreError::new(
                ErrorKind::Unavailable,
                format!(
                    "{} does not exist: is cron installed?",
                    self.cron_d().display()
                ),
            ));
        }
        if let ManagedFile::Unsafe(why) = self.read_managed()?
            && !replace_unsafe
        {
            return Err(CoreError::conflict(format!(
                "refusing to write {}: {why}",
                self.managed_path().display()
            )));
        }
        // Dotted temp name inside /etc/cron.d, the only writable dir under ProtectSystem=strict;
        // cron skips dotted names (5.4, OQ-26).
        let tmp = self.cron_d().join(CRON_TEMP_NAME);
        atomic_write(
            &tmp,
            &self.managed_path(),
            content.as_bytes(),
            0o644,
            Some(self.owner),
        )
    }

    fn system_sources(&self) -> Result<Vec<CronSource>, CoreError> {
        let mut out = Vec::new();
        let crontab = self.etc_dir.join("crontab");
        if crontab.exists() {
            out.push(describe(&crontab, CronSourceKind::Crontab));
        }
        for p in sorted_entries(&self.cron_d()) {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name == CRON_FILE_NAME || name.starts_with('.') {
                continue;
            }
            out.push(describe(&p, CronSourceKind::CronD));
        }
        for period in ["hourly", "daily", "weekly", "monthly"] {
            for p in sorted_entries(&self.etc_dir.join(format!("cron.{period}"))) {
                out.push(CronSource {
                    path: p.display().to_string(),
                    kind: CronSourceKind::Periodic,
                    content: CronContent::NameOnly,
                });
            }
        }
        // Debian keeps user crontabs in `crontabs/`, other distros directly in the spool.
        let debian = self.spool_dir.join("crontabs");
        let spool = if debian.is_dir() {
            debian
        } else {
            self.spool_dir.clone()
        };
        for p in sorted_entries(&spool) {
            let is_file = std::fs::symlink_metadata(&p)
                .map(|m| m.is_file() || m.file_type().is_symlink() || m.file_type().is_fifo())
                .unwrap_or(false);
            if is_file {
                out.push(describe(&p, CronSourceKind::UserSpool));
            }
        }
        Ok(out)
    }

    fn service_active(&self) -> Option<bool> {
        if !self.check_service {
            return None;
        }
        for unit in ["cron", "crond"] {
            match Command::new("systemctl")
                .args(["is-active", "--quiet", unit])
                .status()
            {
                Ok(s) if s.success() => return Some(true),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        Some(false)
    }
}
