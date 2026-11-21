//! `yanto mcp`: MCP over stdio (docs/rewrite/03 d, OQ-16).
//!
//! Runs the services in-process against the shared SQLite database, so it works with
//! `yanto serve` down. Deployments it enqueues stay `queued` until a `serve` process (or its
//! poll fallback) picks them up. Authentication happens once at startup with an API Token
//! read from `YANTO_API_TOKEN_FILE` (a 0600 file) or `YANTO_API_TOKEN`; never from argv,
//! which leaks through `ps`. The session acts as the token's user, capped by its scope.
//! stdout carries only the protocol; logs go to stderr.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::Args;

use super::common::{block_on, load_config, open};

#[derive(Debug, Args)]
pub struct McpArgs {
    /// File holding the API Token (mode 0600). Prefer this over the env var.
    #[arg(long, env = "YANTO_API_TOKEN_FILE", value_name = "PATH")]
    pub token_file: Option<std::path::PathBuf>,
}

pub fn run(config_path: &Path, args: &McpArgs) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    crate::logging::init_stderr(&config.log)?;
    let token = read_token(args.token_file.as_deref(), |k| std::env::var(k).ok())?;
    block_on(async move {
        let rt = open(config).await?;
        let services = Arc::new(rt.services.clone());
        let actor = yanto_server::mcp::authenticate_stdio(&services, &token)
            .await
            .map_err(|e| anyhow::anyhow!("API Token rejected: {}", e.message))?;
        tracing::info!(user = %actor.label, scope = ?actor.token_scope(), "MCP stdio session started");
        let out = yanto_server::mcp::serve_stdio(services, actor).await;
        rt.db.close().await;
        out.context("MCP session")
    })
}

/// The token from the file, else from `YANTO_API_TOKEN`. Errors never contain the value.
fn read_token(file: Option<&Path>, env: impl Fn(&str) -> Option<String>) -> anyhow::Result<String> {
    let raw = if let Some(path) = file {
        check_mode(path)?;
        std::fs::read_to_string(path)
            .with_context(|| format!("reading API Token file {}", path.display()))?
    } else if let Some(v) = env("YANTO_API_TOKEN") {
        v
    } else {
        bail!(
            "an API Token is required: set YANTO_API_TOKEN_FILE (path to a 0600 file, preferred) \
             or YANTO_API_TOKEN; create one in the dashboard under API Tokens"
        );
    };
    let token = raw.trim().to_owned();
    if token.is_empty() {
        bail!("the API Token is empty");
    }
    Ok(token)
}

#[cfg(unix)]
fn check_mode(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        bail!(
            "{} is readable by other users (mode {:o}); run chmod 600 on it",
            path.display(),
            mode & 0o777
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_mode(_: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comes_from_file_then_env_and_is_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t");
        std::fs::write(&f, "yat_abc\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let env = |_: &str| Some("yat_from_env".to_owned());
        assert_eq!(read_token(Some(&f), env).unwrap(), "yat_abc");
        assert_eq!(read_token(None, env).unwrap(), "yat_from_env");
        assert!(read_token(None, |_| None).is_err());
        assert!(read_token(None, |_| Some("  ".into())).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn world_readable_token_files_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t");
        std::fs::write(&f, "yat_abc").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = read_token(Some(&f), |_| None).unwrap_err().to_string();
        assert!(err.contains("chmod 600"), "{err}");
        assert!(!err.contains("yat_abc"));
    }
}
