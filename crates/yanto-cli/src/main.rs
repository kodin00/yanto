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
        cli::Command::SetupCode => cmd::setup_code::run(&cli.config),
        cli::Command::OwnerReset => cmd::owner_reset::run(&cli.config),
        cli::Command::Backup(c) => cmd::backup::run(&cli.config, &c),
        cli::Command::Restore(a) => cmd::backup::restore(&cli.config, &a),
        cli::Command::Mcp(args) => cmd::mcp::run(&cli.config, &args),
        cli::Command::CronRun { id } => cmd::cron_run::run(&cli.config, &id),
        cli::Command::ExecAs(a) => cmd::exec_as::run(&a),
    }
}
