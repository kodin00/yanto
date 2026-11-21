mod cli;
mod cmd;
mod logging;
mod ops;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Command::Version => {
            cmd::version::run();
            Ok(())
        }
        cli::Command::Serve => cmd::serve::run(&cli.config),
        cli::Command::Install(args) => cmd::install::run(&cli.config, &args),
        cli::Command::SelfUpdate(args) => cmd::self_update::run(&cli.config, &args),
        cli::Command::Uninstall(args) => cmd::uninstall::run(&cli.config, &args),
        cli::Command::Doctor(args) => cmd::doctor::run(&cli.config, &args),
    }
}
