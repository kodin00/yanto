//! `yanto uninstall` (docs/rewrite/05 section 8).

use std::path::{Path, PathBuf};

use clap::Args;

use crate::ops::host::RealHost;
use crate::ops::layout::Layout;
use crate::ops::prompt::{InquirePrompter, Prompter};
use crate::ops::uninstall::{UninstallOpts, run as uninstall};

#[derive(Debug, Clone, Args, Default)]
pub struct UninstallArgs {
    /// Also delete the config file and the data dir (DB, backups, keys). Needs --yes or a typed confirmation
    #[arg(long)]
    pub purge: bool,
    /// Remove cron file, authorized_keys block and Traefik file without asking
    #[arg(long, conflicts_with = "keep_managed")]
    pub remove_managed: bool,
    /// Keep cron file, authorized_keys block and Traefik file
    #[arg(long)]
    pub keep_managed: bool,
    /// `docker compose down` every Project (not implemented until Projects exist)
    #[arg(long)]
    pub stop_projects: bool,
    /// Remove the Hosted Registry container (not implemented until M6)
    #[arg(long)]
    pub remove_registry: bool,
    /// Do not prompt; assume yes to confirmations
    #[arg(long)]
    pub yes: bool,
    /// Operate under this directory prefix instead of /
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
}

pub fn run(config_path: &Path, args: &UninstallArgs) -> anyhow::Result<()> {
    let layout = Layout::from_root(args.root.as_deref());
    let managed = match (args.remove_managed, args.keep_managed) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    };
    let opts = UninstallOpts {
        purge: args.purge,
        managed,
        stop_projects: args.stop_projects,
        remove_registry: args.remove_registry,
        yes: args.yes,
    };
    let mut inquire = InquirePrompter;
    let mut prompter: Option<&mut dyn Prompter> = if args.yes { None } else { Some(&mut inquire) };
    if let Some(p) = prompter.as_deref_mut()
        && !p.confirm("Uninstall Yanto? (data is kept unless --purge)", false)?
    {
        anyhow::bail!("aborted");
    }
    let steps = uninstall(
        &RealHost,
        &layout,
        config_path,
        layout.is_system(),
        &opts,
        prompter,
    )?;
    for s in steps {
        println!("  - {s}");
    }
    Ok(())
}
