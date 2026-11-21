//! Per-command `DOCKER_CONFIG` directories on tmpfs (`/run/yanto`, 04 4.7): `config.json`
//! with `auths` only, a `cli-plugins` symlink so `docker compose` is still found, removed
//! when the command ends. `docker login` is never used and nothing is written to `~/.docker`.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use yanto_core::CoreError;
use yanto_core::ports::{AuthEntry, RegistryAuthPort};

#[derive(Debug, Clone)]
pub struct RunDirRegistryAuth {
    base: PathBuf,
}

impl RunDirRegistryAuth {
    /// `base` is `/run/yanto` (systemd `RuntimeDirectory`) or a fallback under the data dir.
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Remove directories a crashed process left behind. Call once at startup.
    pub fn sweep(&self) {
        let Ok(rd) = std::fs::read_dir(&self.base) else {
            return;
        };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("docker-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    fn dir(&self, scope: &str) -> PathBuf {
        self.base.join(format!("docker-{scope}"))
    }
}

fn io(what: &'static str) -> impl FnOnce(std::io::Error) -> CoreError {
    move |e| CoreError::internal(what, e)
}

fn plugin_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    [
        home.map(|h| h.join(".docker").join("cli-plugins")),
        Some("/usr/local/lib/docker/cli-plugins".into()),
        Some("/usr/lib/docker/cli-plugins".into()),
        Some("/usr/libexec/docker/cli-plugins".into()),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.is_dir())
}

#[cfg(unix)]
fn private_dir(path: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(io("cannot create the registry auth directory"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(io("cannot restrict the registry auth directory"))
}

impl RegistryAuthPort for RunDirRegistryAuth {
    fn create(&self, scope: &str, auths: &[AuthEntry]) -> Result<PathBuf, CoreError> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if scope.is_empty() || scope.contains('/') || scope.contains("..") {
            return Err(CoreError::new(
                yanto_core::ErrorKind::Internal,
                "unsafe auth scope",
            ));
        }
        private_dir(&self.base)?;
        let dir = self.dir(scope);
        let _ = std::fs::remove_dir_all(&dir);
        private_dir(&dir)?;
        let mut map = serde_json::Map::new();
        for a in auths {
            let token = STANDARD.encode(format!("{}:{}", a.username, a.secret.expose()));
            map.insert(a.registry_key.clone(), serde_json::json!({ "auth": token }));
        }
        let body = serde_json::json!({ "auths": map }).to_string();
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join("config.json"))
            .map_err(io("cannot write the registry auth file"))?;
        f.write_all(body.as_bytes())
            .and_then(|()| f.sync_all())
            .map_err(io("cannot write the registry auth file"))?;
        if let Some(plugins) = plugin_dir() {
            let _ = std::os::unix::fs::symlink(plugins, dir.join("cli-plugins"));
        }
        Ok(dir)
    }

    fn remove(&self, scope: &str) {
        let _ = std::fs::remove_dir_all(self.dir(scope));
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use yanto_core::secrets::Secret;

    use super::*;

    fn entry() -> AuthEntry {
        AuthEntry {
            registry_key: "ghcr.io".into(),
            username: "bot".into(),
            secret: Secret::new("pw"),
        }
    }

    #[test]
    fn writes_auths_only_privately_and_cleans_up() {
        let base = tempfile::tempdir().unwrap();
        let port = RunDirRegistryAuth::new(base.path().join("run"));
        let dir = port.create("dep_1", &[entry()]).unwrap();
        let cfg = dir.join("config.json");
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&cfg).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let token = v["auths"]["ghcr.io"]["auth"].as_str().unwrap();
        assert_eq!(STANDARD.decode(token).unwrap(), b"bot:pw");
        assert!(v.get("credsStore").is_none() && v.as_object().unwrap().len() == 1);
        port.remove("dep_1");
        assert!(!dir.exists());
        port.remove("dep_1");
    }

    #[test]
    fn sweep_removes_stale_directories_and_scopes_are_checked() {
        let base = tempfile::tempdir().unwrap();
        let port = RunDirRegistryAuth::new(base.path());
        port.create("a", &[entry()]).unwrap();
        port.create("b", &[entry()]).unwrap();
        port.sweep();
        assert!(!base.path().join("docker-a").exists() && !base.path().join("docker-b").exists());
        assert!(port.create("../x", &[entry()]).is_err());
    }
}
