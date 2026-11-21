use std::path::Path;

use yanto_core::actor::Actor;

use super::common::{block_on, load_config, open};

/// Revoke the Owner's sessions and print a one-time password reset link (24 hours).
pub fn run(config_path: &Path) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    block_on(async {
        let rt = open(config).await?;
        let link = rt.services.auth.owner_reset(&Actor::cli()).await?;
        eprintln!(
            "Owner '{}' was signed out everywhere and their password cleared.",
            link.username
        );
        eprintln!(
            "Open this one-time link within 24 hours to choose a new password (treat it as a secret):"
        );
        println!("{}", link.url.expose());
        rt.db.close().await;
        Ok(())
    })
}
