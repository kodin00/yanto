use std::path::Path;

use yanto_core::actor::Actor;

use super::common::{block_on, load_config, open};

/// Issue a new one-time setup code (replacing any earlier one) while no Owner exists.
///
/// The code goes to stdout alone so scripts can capture it; the handling warning goes to stderr.
pub fn run(config_path: &Path) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    block_on(async {
        let rt = open(config).await?;
        let issued = rt.services.auth.issue_setup_code(&Actor::cli()).await?;
        eprintln!("One-time setup code (valid 24 hours, single use). It is a live credential:");
        eprintln!("do not paste it into tickets or chat, and clear your terminal scrollback.");
        println!("{}", issued.code.expose());
        rt.db.close().await;
        Ok(())
    })
}
