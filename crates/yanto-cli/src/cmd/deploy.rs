//! `yanto deploy <slug> [--ref <ref>] [--wait]` (docs/rewrite/05 section 10): enqueue a Deployment
//! of a Project from the Host. The running service executes it; with `--wait` this command polls
//! the database until the Deployment ends and exits non-zero unless it succeeded.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Args;
use yanto_core::actor::Actor;
use yanto_core::service::deployments::DeployRequest;

use super::common::{block_on, load_config, open};

/// Exit code when `--wait` runs out of time (as `timeout(1)`).
pub const EXIT_TIMEOUT: i32 = 124;

#[derive(Debug, Clone, Args)]
pub struct DeployArgs {
    /// Project slug
    pub slug: String,
    /// Git Projects: deploy this branch, tag or commit instead of the branch tip
    #[arg(long = "ref", value_name = "REF")]
    pub git_ref: Option<String>,
    /// Wait until the Deployment finishes; exit 1 unless it succeeded
    #[arg(long)]
    pub wait: bool,
    /// With --wait: give up after this many seconds (exit 124; the Deployment keeps running)
    #[arg(
        long,
        default_value_t = 1800,
        value_name = "SECONDS",
        requires = "wait"
    )]
    pub timeout: u64,
}

fn terminal(status: &str) -> bool {
    matches!(status, "success" | "failed" | "cancelled" | "interrupted")
}

pub fn run(config_path: &Path, args: &DeployArgs) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    let code = block_on(async {
        let rt = open(config).await?;
        let actor = Actor::cli();
        let project = rt
            .services
            .projects
            .list(&actor)
            .await?
            .into_iter()
            .find(|p| p.slug == args.slug)
            .with_context(|| format!("no Project with slug {}", args.slug))?;
        let queued = rt
            .services
            .deployments
            .deploy(
                &actor,
                &project.id,
                &DeployRequest {
                    target_ref: args.git_ref.clone(),
                    env: None,
                },
            )
            .await?;
        let id = queued.deployment.id.clone();
        println!(
            "deployment {id} {}{}",
            queued.deployment.status,
            if queued.reused {
                " (joined the Deployment already queued)"
            } else {
                ""
            }
        );
        let mut code = 0;
        if args.wait {
            let start = Instant::now();
            let mut hinted = false;
            let mut last = String::new();
            loop {
                let d = rt.services.deployments.get(&actor, &id).await?;
                if d.status != last {
                    println!("{id} {}", d.status);
                    last.clone_from(&d.status);
                }
                if terminal(&d.status) {
                    if d.status != "success" {
                        if let Some(e) = &d.error {
                            eprintln!("yanto deploy: {e}");
                        }
                        code = 1;
                    }
                    break;
                }
                if d.status == "queued" && !hinted && start.elapsed() > Duration::from_secs(10) {
                    eprintln!(
                        "yanto deploy: still queued after 10 s; the Deployment starts when `yanto serve` runs"
                    );
                    hinted = true;
                }
                if start.elapsed() > Duration::from_secs(args.timeout) {
                    eprintln!("yanto deploy: gave up waiting after {} s", args.timeout);
                    code = EXIT_TIMEOUT;
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        rt.db.close().await;
        anyhow::Ok(code)
    })?;
    if code != 0 {
        bail_exit(code);
    }
    Ok(())
}

fn bail_exit(code: i32) -> ! {
    std::process::exit(code)
}
