//! Binary swap, DB pre-update copy and rollback for `self-update` (05 section 7).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::{Context, bail};

use super::dbcopy;
use super::layout::{BIN_PATH, Layout};

pub fn bin(layout: &Layout) -> PathBuf {
    layout.path(BIN_PATH)
}

pub fn prev(layout: &Layout) -> PathBuf {
    layout.path(format!("{BIN_PATH}.prev"))
}

fn new_path(layout: &Layout) -> PathBuf {
    layout.path(format!("{BIN_PATH}.new"))
}

/// Install `bytes` as the new binary: keep the current one as `.prev`, then
/// atomically rename the new one over it (a running executable can be
/// replaced by rename on Linux).
pub fn swap_in(layout: &Layout, bytes: &[u8]) -> anyhow::Result<()> {
    let (cur, prv, new) = (bin(layout), prev(layout), new_path(layout));
    let dir = cur.parent().context("binary path has no parent")?;
    fs::create_dir_all(dir)?;
    fs::write(&new, bytes).with_context(|| format!("writing {}", new.display()))?;
    fs::set_permissions(&new, fs::Permissions::from_mode(0o755))?;
    if cur.exists() {
        let _ = fs::remove_file(&prv);
        fs::hard_link(&cur, &prv)
            .or_else(|_| fs::copy(&cur, &prv).map(|_| ()))
            .with_context(|| format!("preserving {}", prv.display()))?;
    }
    fs::rename(&new, &cur).with_context(|| format!("replacing {}", cur.display()))?;
    Ok(())
}

/// Put `.prev` back as the live binary.
pub fn restore_prev(layout: &Layout) -> anyhow::Result<()> {
    let (cur, prv) = (bin(layout), prev(layout));
    if !prv.exists() {
        bail!("{} does not exist; nothing to roll back to", prv.display());
    }
    let tmp = cur.with_extension("restore");
    fs::copy(&prv, &tmp)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    fs::rename(&tmp, &cur)?;
    Ok(())
}

/// Copy the DB to `backups/yanto/pre-update-<ts>.db`; `None` if there is no DB yet.
pub fn backup_db(layout: &Layout, data_dir: &std::path::Path) -> anyhow::Result<Option<PathBuf>> {
    let db = layout.path(data_dir).join("yanto.db");
    if !db.exists() {
        return Ok(None);
    }
    let dest = layout.path(data_dir).join(format!(
        "backups/yanto/pre-update-{}.db",
        dbcopy::now_stamp()
    ));
    dbcopy::backup(&db, &dest)?;
    Ok(Some(dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> (tempfile::TempDir, Layout) {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path());
        fs::create_dir_all(bin(&l).parent().unwrap()).unwrap();
        (d, l)
    }

    #[test]
    fn swap_keeps_prev_and_rollback_restores() {
        let (_d, l) = layout();
        fs::write(bin(&l), b"old").unwrap();
        swap_in(&l, b"new").unwrap();
        assert_eq!(fs::read(bin(&l)).unwrap(), b"new");
        assert_eq!(fs::read(prev(&l)).unwrap(), b"old");
        assert_eq!(
            fs::metadata(bin(&l)).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!new_path(&l).exists());
        restore_prev(&l).unwrap();
        assert_eq!(fs::read(bin(&l)).unwrap(), b"old");
    }

    #[test]
    fn rollback_without_prev_errors() {
        let (_d, l) = layout();
        assert!(restore_prev(&l).is_err());
    }

    #[test]
    fn second_swap_replaces_prev() {
        let (_d, l) = layout();
        fs::write(bin(&l), b"v1").unwrap();
        swap_in(&l, b"v2").unwrap();
        swap_in(&l, b"v3").unwrap();
        assert_eq!(fs::read(prev(&l)).unwrap(), b"v2");
    }

    #[test]
    fn db_backup_lands_under_backups_yanto() {
        let (_d, l) = layout();
        let data = std::path::Path::new("/var/lib/yanto");
        assert!(backup_db(&l, data).unwrap().is_none());
        let db = l.path(data).join("yanto.db");
        crate::ops::install::migrate(&db).unwrap();
        let dest = backup_db(&l, data).unwrap().unwrap();
        assert!(dest.starts_with(l.path("/var/lib/yanto/backups/yanto")));
        assert!(
            dest.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pre-update-")
        );
    }
}
