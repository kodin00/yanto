//! Filesystem prefixing so every system path can be redirected into a temp dir.

use std::path::{Path, PathBuf};

pub const BIN_PATH: &str = "/usr/local/bin/yanto";
pub const UNIT_PATH: &str = "/etc/systemd/system/yanto.service";
pub const UNIT_NAME: &str = "yanto";
pub const CRON_PATH: &str = "/etc/cron.d/yanto";
pub const OS_RELEASE: &str = "/etc/os-release";
pub const PASSWD: &str = "/etc/passwd";
pub const SYSTEMD_RUN_DIR: &str = "/run/systemd/system";

/// Maps logical absolute paths (`/etc/yanto/config.toml`) to real ones
/// (`<root>/etc/yanto/config.toml`). With root `/` it is the identity.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn from_root(root: Option<&Path>) -> Self {
        root.map_or_else(Self::system, Self::new)
    }

    pub fn system() -> Self {
        Self::new("/")
    }

    /// True when operating on the real system root (systemctl etc. allowed).
    pub fn is_system(&self) -> bool {
        self.root == Path::new("/")
    }

    /// Real path for a logical absolute path.
    pub fn path(&self, logical: impl AsRef<Path>) -> PathBuf {
        let logical = logical.as_ref();
        if self.is_system() {
            return logical.to_path_buf();
        }
        self.root.join(logical.strip_prefix("/").unwrap_or(logical))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_layout_is_identity() {
        assert_eq!(Layout::system().path("/etc/x"), PathBuf::from("/etc/x"));
    }

    #[test]
    fn prefixed_layout_roots_absolute_paths() {
        let l = Layout::new("/tmp/r");
        assert!(!l.is_system());
        assert_eq!(l.path("/etc/x"), PathBuf::from("/tmp/r/etc/x"));
        assert_eq!(l.path("rel"), PathBuf::from("/tmp/r/rel"));
    }
}
