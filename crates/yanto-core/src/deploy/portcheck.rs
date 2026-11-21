//! Gathers everything the port collision check compares against and runs it
//! (docs/rewrite/04 1.1): other Projects' declared ports, containers published by other
//! Compose projects, and sockets bound on the Host.

use std::sync::Arc;

use super::compose::{self, CollisionInputs, Conflict, DesiredPort};
use super::engine::Engine;
use crate::db::repo::{db_err, projects};
use crate::error::CoreError;
use crate::ports::{CancellationToken, CommandSpec, NullSink};

pub struct PortCheck {
    pub conflicts: Vec<Conflict>,
    /// `docker ps` could not be read, so the container part of the check was skipped.
    pub containers_unchecked: bool,
}

pub async fn check(
    eng: &Engine,
    project_id: &str,
    slug: &str,
    desired: &[DesiredPort],
    cancel: &CancellationToken,
) -> Result<PortCheck, CoreError> {
    if desired.is_empty() {
        return Ok(PortCheck {
            conflicts: Vec::new(),
            containers_unchecked: false,
        });
    }
    let ps = eng
        .host
        .command
        .run(
            CommandSpec::new(
                "docker",
                &[
                    "ps",
                    "--format",
                    "{{.Names}}\t{{index .Labels \"com.docker.compose.project\"}}\t{{.Ports}}",
                ],
            )
            .capture()
            .timeout(std::time::Duration::from_secs(60)),
            Arc::new(NullSink),
            cancel,
        )
        .await;
    let (containers, containers_unchecked) = match ps {
        Ok(o) if o.exit_code == 0 => (compose::parse_ps_ports(&o.stdout), false),
        _ => (Vec::new(), true),
    };
    let listeners = eng.host.probe.listeners().await?;
    let others: Vec<(String, u16, String)> = {
        let mut conn = eng.ctx.db.reader().acquire().await.map_err(db_err)?;
        projects::ports_of_others(&mut conn, project_id)
            .await?
            .into_iter()
            .filter_map(|(name, r)| Some((name, u16::try_from(r.host_port).ok()?, r.protocol)))
            .collect()
    };
    let conflicts = compose::find_conflicts(&CollisionInputs {
        slug,
        desired,
        other_projects: &others,
        containers: &containers,
        listeners: &listeners,
    });
    Ok(PortCheck {
        conflicts,
        containers_unchecked,
    })
}

pub fn describe(conflicts: &[Conflict]) -> String {
    conflicts
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}
