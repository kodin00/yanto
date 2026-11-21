//! `yanto backup ...` and `yanto restore yanto` (docs/rewrite/04 sections 7.5 and 7.6).
//!
//! Backups run in-process against the same data dir and take the series lock, so a second run
//! exits 75. Meant for Cron Entries: non-interactive, one summary line, non-zero exit on failure.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use yanto_core::actor::{Actor, Channel};
use yanto_core::backup::{self, BackupKind, Trigger, Verify};
use yanto_core::service::backups::{BUSY_MESSAGE, Retention};
use yanto_core::{ErrorKind, secrets};

use super::common::{block_on, load_config, open};

/// Exit code when another backup of the series holds the lock.
pub const EXIT_BUSY: i32 = 75;

#[derive(Debug, Clone, Args)]
pub struct Retain {
    /// Keep this many scheduled backups of the series (default 7 when neither flag is given to a scheduled run)
    #[arg(long, value_name = "N")]
    pub keep_last: Option<u32>,
    /// Delete scheduled backups older than this many days
    #[arg(long, value_name = "D")]
    pub keep_days: Option<u32>,
}

impl Retain {
    fn policy(&self) -> Retention {
        Retention {
            keep_last: self.keep_last,
            keep_days: self.keep_days,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum BackupCommand {
    /// Online backup of Yanto's own database, bundled with keys/app.key (D22)
    Yanto {
        #[command(flatten)]
        retain: Retain,
        /// Print the full backup record
        #[arg(short, long)]
        verbose: bool,
    },
    /// Back up a Project's named volumes or one declared database (04 7.5)
    Project {
        slug: String,
        /// Archive every named volume of the Project (one .tar.zst each)
        #[arg(long, conflicts_with = "db", required_unless_present = "db")]
        volumes: bool,
        /// Dump the declared database of this compose service
        #[arg(long, value_name = "SERVICE")]
        db: Option<String>,
        /// Stop the Project while archiving volumes (started again afterwards)
        #[arg(long, requires = "volumes")]
        stop_project: bool,
        #[command(flatten)]
        retain: Retain,
    },
    /// List backups
    List,
    /// Delete scheduled backups beyond the retention flags
    Prune {
        #[command(flatten)]
        retain: Retain,
    },
}

#[derive(Debug, Clone, Args)]
pub struct RestoreArgs {
    /// What to restore (only `yanto`)
    #[arg(value_parser = ["yanto"])]
    pub what: String,
    /// Backup archive (`<ts>-yanto.tar.zst`) with its `.sha256` file next to it
    pub archive: PathBuf,
    /// Do not ask for confirmation
    #[arg(long)]
    pub yes: bool,
    /// Restore an archive that has no .sha256 file (explicit operator choice)
    #[arg(long)]
    pub allow_unverified: bool,
    /// Skip the check that the yanto service is stopped (tests, containers without systemd)
    #[arg(long, hide = true)]
    pub assume_stopped: bool,
}

fn cli_actor() -> (Actor, bool) {
    let mut a = Actor::cli();
    let from_cron = std::env::var_os("YANTO_CRON_RUN").is_some();
    if from_cron {
        a.channel = Channel::Cron;
    }
    (a, from_cron)
}

fn require_key(data_dir: &Path) -> anyhow::Result<()> {
    if !secrets::key_path(data_dir).is_file() {
        bail!(
            "{} not found: refusing to run without the instance key (nothing was created)",
            secrets::key_path(data_dir).display()
        );
    }
    Ok(())
}

pub fn run(config_path: &Path, cmd: &BackupCommand) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    require_key(&config.paths.data_dir)?;
    let code = block_on(async {
        let rt = open(config).await?;
        let (actor, from_cron) = cli_actor();
        let result = match cmd {
            BackupCommand::Yanto { retain, verbose } => {
                let scheduled =
                    from_cron || retain.keep_last.is_some() || retain.keep_days.is_some();
                let trigger = if scheduled {
                    Trigger::Cron
                } else {
                    Trigger::Manual
                };
                match rt
                    .services
                    .backups
                    .create_yanto(&actor, trigger, Some(retain.policy()))
                    .await
                {
                    Ok(b) => {
                        println!(
                            "backup {} {} {} bytes sha256:{}",
                            b.id,
                            b.filename,
                            b.size_bytes.unwrap_or(0),
                            b.sha256.clone().unwrap_or_default()
                        );
                        if *verbose {
                            println!("{b:#?}");
                        }
                        Ok(0)
                    }
                    Err(e) if e.kind == ErrorKind::Conflict && e.message == BUSY_MESSAGE => {
                        eprintln!("yanto backup: busy: {}", e.message);
                        Ok(EXIT_BUSY)
                    }
                    Err(e) => Err(anyhow::Error::new(e)),
                }
            }
            BackupCommand::Project {
                slug,
                volumes,
                db,
                stop_project,
                retain,
            } => {
                let scheduled =
                    from_cron || retain.keep_last.is_some() || retain.keep_days.is_some();
                let trigger = if scheduled {
                    Trigger::Cron
                } else {
                    Trigger::Manual
                };
                run_project(
                    &rt,
                    &actor,
                    slug,
                    *volumes,
                    db.as_deref(),
                    *stop_project,
                    trigger,
                    retain.policy(),
                )
                .await
            }
            BackupCommand::List => {
                for b in rt.services.backups.list(&actor).await? {
                    println!(
                        "{}  {:<8} {:<15} {:<11} {:>10}  {}",
                        b.id,
                        b.status,
                        b.kind,
                        b.trigger,
                        b.size_bytes.unwrap_or(0),
                        b.filename
                    );
                }
                Ok(0)
            }
            BackupCommand::Prune { retain } => {
                let n = rt.services.backups.prune(&actor, retain.policy()).await?;
                println!("deleted {n} backup(s)");
                Ok(0)
            }
        };
        rt.db.close().await;
        result
    })?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

pub fn restore(config_path: &Path, args: &RestoreArgs) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    if !args.assume_stopped {
        ensure_daemon_stopped()?;
    }
    if !args.yes {
        eprintln!(
            "This replaces {} and keys/app.key with the contents of {}.",
            config.db_path().display(),
            args.archive.display()
        );
        eprintln!(
            "The current files are kept as *.pre-restore-<timestamp>. Sessions and API Tokens revert to the backup's time."
        );
        if !crate::ops::prompt::Prompter::confirm(
            &mut crate::ops::prompt::InquirePrompter,
            "Restore now?",
            false,
        )? {
            bail!("aborted");
        }
    }
    let verify = if args.allow_unverified {
        Verify::IfPresent
    } else {
        Verify::Required
    };
    let now = yanto_core::ports::Clock::now_ms(&yanto_core::ports::SystemClock);
    let report = block_on(async {
        backup::restore_yanto(&config.paths.data_dir, &args.archive, now, verify)
            .await
            .map_err(anyhow::Error::new)
    })
    .context("restore failed; nothing was replaced unless stated above")?;
    if let Some(p) = &report.previous_db {
        println!("previous database kept as {}", p.display());
    }
    if let Some(p) = &report.previous_key {
        println!("previous app.key kept as {}", p.display());
    }
    println!("restored. Start the service again: systemctl start yanto");
    Ok(())
}

/// Refuse while `systemctl` says the yanto unit is active (7.6). No systemctl: nothing to check.
fn ensure_daemon_stopped() -> anyhow::Result<()> {
    match std::process::Command::new("systemctl")
        .args(["is-active", "--quiet", "yanto"])
        .status()
    {
        Ok(s) if s.success() => bail!(
            "the yanto service is running; stop it first: systemctl stop yanto && yanto restore yanto <archive> && systemctl start yanto"
        ),
        _ => Ok(()),
    }
}

/// `yanto backup project <slug> (--volumes | --db <service>)`: print one line per archive.
#[allow(clippy::too_many_arguments)]
async fn run_project(
    rt: &super::common::Runtime,
    actor: &Actor,
    slug: &str,
    volumes: bool,
    db: Option<&str>,
    stop_project: bool,
    trigger: Trigger,
    retention: Retention,
) -> anyhow::Result<i32> {
    let project = rt
        .services
        .projects
        .list(actor)
        .await?
        .into_iter()
        .find(|p| p.slug == slug)
        .with_context(|| format!("no Project with slug {slug}"))?;
    let (kind, target) = match (volumes, db) {
        (true, _) => (BackupKind::ProjectVolumes, None),
        (false, Some(service)) => {
            let decl = rt
                .services
                .projects
                .databases_list(actor, &project.id)
                .await?
                .into_iter()
                .find(|d| d.compose_service == service)
                .with_context(|| {
                    format!("no database is declared for service {service} on {slug}")
                })?;
            let kind = BackupKind::parse(&decl.engine)
                .with_context(|| format!("unknown engine {}", decl.engine))?;
            (kind, Some(service))
        }
        (false, None) => bail!("pass --volumes or --db <service>"),
    };
    match rt
        .services
        .backups
        .create_project_with(
            actor,
            &project.id,
            kind,
            target,
            stop_project,
            trigger,
            Some(retention),
        )
        .await
    {
        Ok(list) => {
            for b in list {
                println!(
                    "backup {} {} {} bytes sha256:{}",
                    b.id,
                    b.filename,
                    b.size_bytes.unwrap_or(0),
                    b.sha256.unwrap_or_default()
                );
            }
            Ok(0)
        }
        Err(e) if e.kind == ErrorKind::Conflict && e.message == BUSY_MESSAGE => {
            eprintln!("yanto backup: busy: {}", e.message);
            Ok(EXIT_BUSY)
        }
        Err(e) => Err(anyhow::Error::new(e)),
    }
}
