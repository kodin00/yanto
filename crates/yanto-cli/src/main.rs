mod cli;
mod cmd;
mod logging;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Command::Version => {
            cmd::version::run();
            Ok(())
        }
        cli::Command::Serve => cmd::serve::run(&cli.config),
        cli::Command::SetupCode => cmd::setup_code::run(&cli.config),
        cli::Command::OwnerReset => cmd::owner_reset::run(&cli.config),
    }
}
