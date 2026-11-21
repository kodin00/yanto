//! `frps` adapters: the `frps.toml` file (backup, atomic replace keeping mode and owner) and the
//! dashboard HTTP API (`GET`, Basic auth, 2 s timeout).

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use yanto_core::CoreError;
use yanto_core::ports::{DashboardTarget, FrpDashboardPort, FrpFile, FrpFilesPort, HttpAnswer};

use crate::fsutil::{atomic_write, io_err};

#[derive(Debug, Default, Clone, Copy)]
pub struct HostFrpFiles;

fn sibling(path: &Path, name: String) -> PathBuf {
    path.parent().unwrap_or_else(|| Path::new("/")).join(name)
}

fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".yanto.bak");
    PathBuf::from(s)
}

impl FrpFilesPort for HostFrpFiles {
    fn read(&self, path: &Path) -> Result<Option<FrpFile>, CoreError> {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err("cannot read frps.toml", e)),
        };
        let content =
            std::fs::read_to_string(path).map_err(|e| io_err("cannot read frps.toml", e))?;
        Ok(Some(FrpFile {
            content,
            mode: meta.mode() & 0o7777,
        }))
    }

    fn write(&self, path: &Path, content: &str) -> Result<(), CoreError> {
        let name = path
            .file_name()
            .map_or_else(|| "frps.toml".into(), |n| n.to_string_lossy().into_owned());
        let meta = std::fs::metadata(path).map_err(|e| io_err("cannot stat frps.toml", e))?;
        // The old content goes to `<path>.yanto.bak` (0600) first.
        let old = std::fs::read(path).map_err(|e| io_err("cannot read frps.toml", e))?;
        let bak = backup_path(path);
        atomic_write(
            &sibling(path, format!(".{name}.yanto.bak.tmp")),
            &bak,
            &old,
            0o600,
            None,
        )?;
        atomic_write(
            &sibling(path, format!(".{name}.yanto.tmp")),
            path,
            content.as_bytes(),
            meta.mode() & 0o7777,
            Some((meta.uid(), meta.gid())),
        )
    }

    fn read_backup(&self, path: &Path) -> Result<Option<String>, CoreError> {
        match std::fs::read_to_string(backup_path(path)) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err("cannot read the frps.toml backup", e)),
        }
    }
}

pub struct FrpDashboardHttp {
    client: Client,
}

impl Default for FrpDashboardHttp {
    fn default() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(2))
                .redirect(reqwest::redirect::Policy::none())
                // Loopback frps dashboard with Basic auth: never route it via HTTP(S)_PROXY.
                .no_proxy()
                .build()
                .expect("reqwest client"),
        }
    }
}

#[async_trait]
impl FrpDashboardPort for FrpDashboardHttp {
    async fn get(&self, t: &DashboardTarget, path: &str) -> Result<HttpAnswer, CoreError> {
        let host = if t.addr.contains(':') {
            format!("[{}]", t.addr)
        } else {
            t.addr.clone()
        };
        let mut req = self.client.get(format!("http://{host}:{}{path}", t.port));
        if let Some((user, pw)) = &t.auth {
            req = req.basic_auth(user, Some(pw.expose()));
        }
        let resp = req.send().await.map_err(|e| {
            CoreError::new(
                yanto_core::ErrorKind::Unavailable,
                format!("frps dashboard unreachable: {}", e.without_url()),
            )
        })?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Ok(HttpAnswer { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use yanto_core::secrets::Secret;

    #[test]
    fn write_keeps_mode_makes_a_private_backup_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("frps.toml");
        std::fs::write(&p, "bindPort = 7000\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        let f = HostFrpFiles;
        assert_eq!(f.read_backup(&p).unwrap(), None);
        f.write(&p, "bindPort = 7001\n").unwrap();
        let got = f.read(&p).unwrap().unwrap();
        assert_eq!(got.content, "bindPort = 7001\n");
        assert_eq!(got.mode, 0o640);
        assert_eq!(
            f.read_backup(&p).unwrap().as_deref(),
            Some("bindPort = 7000\n")
        );
        let bak_mode = std::fs::metadata(dir.path().join("frps.toml.yanto.bak"))
            .unwrap()
            .mode()
            & 0o7777;
        assert_eq!(bak_mode, 0o600);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.contains(".tmp")), "{names:?}");
        assert_eq!(
            f.read(&dir.path().join("none.toml"))
                .unwrap()
                .map(|x| x.content),
            None
        );
    }

    #[tokio::test]
    async fn dashboard_get_uses_basic_auth() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/serverinfo"))
            .and(header("authorization", "Basic YWRtaW46aHVudGVyMg=="))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"version\":\"1\"}"))
            .expect(1)
            .mount(&server)
            .await;
        let addr = server.address();
        let target = DashboardTarget {
            addr: addr.ip().to_string(),
            port: addr.port(),
            auth: Some(("admin".into(), Secret::new("hunter2"))),
        };
        let a = FrpDashboardHttp::default()
            .get(&target, "/api/serverinfo")
            .await
            .unwrap();
        assert_eq!(a.status, 200);
        let dead = DashboardTarget {
            addr: "127.0.0.1".into(),
            port: 1,
            auth: None,
        };
        assert!(FrpDashboardHttp::default().get(&dead, "/").await.is_err());
    }
}
