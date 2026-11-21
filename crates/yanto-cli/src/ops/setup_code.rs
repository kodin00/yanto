//! One-time setup code for a fresh install (05 section 3.6, D18), through the auth service so the
//! code is the same hashed, 24 h, single-use credential `yanto setup-code` issues. The installer
//! only decides whether to issue one and prints the plaintext exactly once.

use std::path::Path;

use anyhow::Context;
use yanto_core::Config;
use yanto_core::actor::Actor;

use crate::cmd::common::open;

fn config_for(data_dir: &Path) -> Config {
    let mut config = Config::default();
    config.paths.data_dir = data_dir.to_path_buf();
    config
}

fn block_on<T>(f: impl std::future::Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(f)
}

/// True when an Owner already exists in the database under `data_dir`.
pub fn owner_exists(data_dir: &Path) -> anyhow::Result<bool> {
    block_on(async {
        let rt = open(config_for(data_dir)).await?;
        let pending = rt.services.auth.setup_pending().await;
        rt.db.close().await;
        Ok(!pending.context("checking for an Owner")?)
    })
}

/// Issue a setup code (replacing any earlier one) and return the plaintext to print once.
pub fn issue(data_dir: &Path) -> anyhow::Result<Option<String>> {
    block_on(async {
        let rt = open(config_for(data_dir)).await?;
        let issued = rt.services.auth.issue_setup_code(&Actor::cli()).await;
        rt.db.close().await;
        let issued = issued.context("issuing the setup code")?;
        Ok(Some(issued.code.expose().to_owned()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // `apply` creates the key and migrates before asking for a code; do the same here.
        std::fs::create_dir_all(dir.path().join("keys")).unwrap();
        dir
    }

    #[test]
    fn issues_a_real_code_while_no_owner_exists() {
        let dir = data_dir();
        assert!(!owner_exists(dir.path()).unwrap());
        let code = issue(dir.path()).unwrap().unwrap();
        assert_eq!(code.len(), 19);
        assert_eq!(code.split('-').count(), 4);
        let again = issue(dir.path()).unwrap().unwrap();
        assert_ne!(code, again, "a new code replaces the old one");
    }

    #[test]
    fn issued_code_creates_the_owner_and_then_owner_exists() {
        use std::sync::Arc;
        use yanto_core::actor::{Channel, Origin};
        let dir = data_dir();
        let code = issue(dir.path()).unwrap().unwrap();
        block_on(async {
            let rt = open(config_for(dir.path())).await?;
            rt.services
                .auth
                .owner_setup(
                    &Origin::new(Channel::Dashboard, None),
                    &code,
                    "boss",
                    "a-long-password-1",
                    None,
                )
                .await?;
            rt.db.close().await;
            let _ = Arc::new(());
            Ok(())
        })
        .unwrap();
        assert!(owner_exists(dir.path()).unwrap());
        // With an Owner, issuing is refused (use `yanto owner-reset`).
        assert!(issue(dir.path()).is_err());
    }
}
