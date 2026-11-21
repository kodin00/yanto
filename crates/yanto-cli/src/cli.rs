use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "yanto",
    about = "Self-hosted VPS management",
    disable_version_flag = true
)]
pub struct Cli {
    /// Path to config.toml
    #[arg(
        long,
        global = true,
        env = "YANTO_CONFIG",
        default_value = yanto_core::config::DEFAULT_CONFIG_PATH
    )]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// Run the dashboard and API server
    Serve,
    /// Issue a one-time setup code for creating the first Owner (while none exists)
    SetupCode,
    /// Revoke the Owner's sessions and print a one-time password reset link
    OwnerReset,
    /// Print the version
    Version,
    /// Interactive/non-interactive installer
    Install(crate::cmd::install::Args),
    /// Update the binary from GitHub Releases
    SelfUpdate(crate::cmd::self_update::SelfUpdateArgs),
    /// Remove Yanto (keeps data unless --purge)
    Uninstall(crate::cmd::uninstall::UninstallArgs),
    /// Re-run prerequisite, Host Daemon and path checks
    Doctor(crate::cmd::doctor::DoctorArgs),
    /// Create, list and prune Backups (used by Cron Entries)
    #[command(subcommand)]
    Backup(crate::cmd::backup::BackupCommand),
    /// Restore the Yanto database from a Backup (service must be stopped)
    Restore(crate::cmd::backup::RestoreArgs),
    /// MCP server over stdio (API Token from YANTO_API_TOKEN_FILE or YANTO_API_TOKEN)
    Mcp(crate::cmd::mcp::McpArgs),
    /// Run a Cron Entry (called by /etc/cron.d/yanto)
    CronRun {
        /// Cron Entry id
        id: String,
    },
    /// Internal: drop privileges and exec (used by cron-run)
    #[command(hide = true)]
    ExecAs(crate::cmd::exec_as::ExecAsArgs),
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn identity_subcommands_parse() {
        let cli = Cli::try_parse_from(["yanto", "setup-code", "--config", "/tmp/c.toml"]).unwrap();
        assert!(matches!(cli.command, Command::SetupCode));
        let cli = Cli::try_parse_from(["yanto", "owner-reset"]).unwrap();
        assert!(matches!(cli.command, Command::OwnerReset));
    }

    #[test]
    fn mcp_subcommand_parses_without_a_token_argument() {
        let cli = Cli::try_parse_from(["yanto", "mcp"]).unwrap();
        assert!(matches!(cli.command, Command::Mcp(_)));
        // The token itself is never an argument.
        assert!(Cli::try_parse_from(["yanto", "mcp", "--token", "x"]).is_err());
    }

    #[test]
    fn config_flag_is_global() {
        let cli = Cli::try_parse_from(["yanto", "serve", "--config", "/tmp/x.toml"]).unwrap();
        assert_eq!(cli.config, PathBuf::from("/tmp/x.toml"));
        assert!(matches!(cli.command, Command::Serve));
    }
}
