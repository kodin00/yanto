//! `yanto cron-run <id>`: called only by `/etc/cron.d/yanto` (docs/rewrite/04 section 5.3).
//!
//! Runs the entry's command as its `run_as_user`, records exit code, duration and output tail.
//! Fails closed with exit 75 when the database cannot be opened: without it the command is unknown.

use std::path::Path;
use std::process::Command;

use yanto_core::actor::Actor;

use super::common::{block_on, load_config, open};

/// Exit code when the database is unreadable (`EX_TEMPFAIL`).
pub const EXIT_UNAVAILABLE: i32 = 75;
/// Exit code for a command that hit its timeout (as `timeout(1)`).
pub const EXIT_TIMEOUT: i32 = 124;

pub fn run(config_path: &Path, id: &str) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    if !config.db_path().is_file() {
        return unavailable(&format!(
            "database {} does not exist; not running {id}",
            config.db_path().display()
        ));
    }
    let code = block_on(async {
        let rt = match open(config).await {
            Ok(rt) => rt,
            Err(e) => {
                syslog(&format!("cron-run {id}: cannot open the database: {e:#}"));
                return Ok(EXIT_UNAVAILABLE);
            }
        };
        let mut actor = Actor::cli();
        actor.channel = yanto_core::actor::Channel::Cron;
        let report = rt.services.cron.run_entry(&actor, id).await;
        rt.db.close().await;
        let report = report?;
        println!(
            "cron-run {id}: {} (exit {}) in {} ms",
            report.status,
            report
                .exit_code
                .map_or_else(|| "-".to_owned(), |c| c.to_string()),
            report.duration_ms
        );
        Ok(match report.status.as_str() {
            "success" | "skipped" => 0,
            "timeout" => EXIT_TIMEOUT,
            _ => report.exit_code.filter(|c| *c != 0).unwrap_or(1),
        })
    })?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

fn unavailable(msg: &str) -> anyhow::Result<()> {
    syslog(&format!("cron-run: {msg}"));
    std::process::exit(EXIT_UNAVAILABLE)
}

/// stderr (journald under systemd) plus a best-effort `logger` call, since cron drops output.
fn syslog(msg: &str) {
    eprintln!("yanto: {msg}");
    let _ = Command::new("logger")
        .args(["-t", "yanto-cron", msg])
        .status();
}
