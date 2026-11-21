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
pub enum Command {
    /// Run the dashboard and API server
    Serve,
    /// Print the version
    Version,
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
    fn config_flag_is_global() {
        let cli = Cli::try_parse_from(["yanto", "serve", "--config", "/tmp/x.toml"]).unwrap();
        assert_eq!(cli.config, PathBuf::from("/tmp/x.toml"));
        assert!(matches!(cli.command, Command::Serve));
    }
}
