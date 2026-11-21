//! Docker Engine over the local socket, via bollard (docs/rewrite/01 7.1).
//!
//! Connection is lazy: building a [`BollardDocker`] never touches the socket, so `yanto install`
//! and the CLI work on hosts where Docker is down. The first request negotiates the API
//! version; a failed connect is not cached, so a daemon that starts later is picked up.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{ContainerInspectResponse, ContainerSummary};
use bollard::query_parameters::{
    ListContainersOptionsBuilder, ListImagesOptionsBuilder, ListNetworksOptionsBuilder,
    LogsOptionsBuilder, PruneBuildOptionsBuilder, PruneImagesOptionsBuilder,
    RestartContainerOptionsBuilder, StatsOptionsBuilder, StopContainerOptionsBuilder,
};
use futures_util::{StreamExt, TryStreamExt};
use tokio::sync::OnceCell;
use yanto_core::ports::{
    BoxStream, ContainerDetail, ContainerInfo, ContainerStats, DiskUsage, DockerError, DockerInfo,
    DockerPort, ExecIo, ExecState, LogLine, LogStream, MountInfo, PortInfo, PruneCandidate,
    PruneKind, PruneOutcome, PrunePreviewRaw, UsageBucket,
};

const CONNECT_TIMEOUT_SECS: u64 = 30;
const SHELL_COMMAND: &str = "command -v bash >/dev/null && exec bash || exec sh";

struct Inner {
    socket: String,
    client: OnceCell<Docker>,
}

impl Inner {
    async fn docker(&self) -> Result<&Docker, DockerError> {
        self.client
            .get_or_try_init(|| async {
                let d = Docker::connect_with_unix(
                    &self.socket,
                    CONNECT_TIMEOUT_SECS,
                    bollard::API_DEFAULT_VERSION,
                )
                .map_err(map_err)?;
                d.negotiate_version().await.map_err(map_err)
            })
            .await
    }
}

pub struct BollardDocker {
    inner: std::sync::Arc<Inner>,
}

impl BollardDocker {
    pub fn new(socket: &Path) -> Self {
        Self {
            inner: std::sync::Arc::new(Inner {
                socket: socket.to_string_lossy().into_owned(),
                client: OnceCell::new(),
            }),
        }
    }

    async fn docker(&self) -> Result<&Docker, DockerError> {
        self.inner.docker().await
    }
}

fn map_err(e: bollard::errors::Error) -> DockerError {
    use bollard::errors::Error as E;
    match e {
        E::DockerResponseServerError {
            status_code,
            message,
        } => match status_code {
            404 => DockerError::NotFound,
            304 => DockerError::Other("not modified".into()),
            409 => DockerError::Conflict(message),
            _ => DockerError::Other(format!("{status_code}: {message}")),
        },
        other => DockerError::Unavailable(other.to_string()),
    }
}

fn clean_name(n: &str) -> String {
    n.trim_start_matches('/').to_owned()
}

fn summary_to_info(c: ContainerSummary) -> ContainerInfo {
    ContainerInfo {
        id: c.id.unwrap_or_default(),
        name: c
            .names
            .and_then(|n| n.into_iter().next())
            .map(|n| clean_name(&n))
            .unwrap_or_default(),
        image: c.image.unwrap_or_default(),
        state: c.state.map(|s| s.to_string()).unwrap_or_default(),
        status: c.status.unwrap_or_default(),
        created: c.created.unwrap_or_default(),
        ports: c
            .ports
            .unwrap_or_default()
            .into_iter()
            .map(|p| PortInfo {
                ip: p.ip,
                private_port: p.private_port,
                public_port: p.public_port,
                proto: p.typ.map(|t| t.to_string()).unwrap_or_else(|| "tcp".into()),
            })
            .collect(),
        labels: c.labels.unwrap_or_default().into_iter().collect(),
    }
}

fn inspect_to_detail(r: ContainerInspectResponse, fallback_id: &str) -> ContainerDetail {
    let state = r.state.unwrap_or_default();
    let config = r.config.unwrap_or_default();
    let ns = r.network_settings.unwrap_or_default();
    let created = r
        .created
        .map(|d| d.to_string())
        .and_then(|s| chrono_like_unix(&s))
        .unwrap_or_default();
    let info = ContainerInfo {
        id: r.id.unwrap_or_else(|| fallback_id.to_owned()),
        name: clean_name(&r.name.unwrap_or_default()),
        image: config.image.clone().unwrap_or_default(),
        state: state.status.map(|s| s.to_string()).unwrap_or_default(),
        status: String::new(),
        created,
        ports: vec![],
        labels: config
            .labels
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
    };
    let mut cmd = r.path.unwrap_or_default();
    for a in r.args.unwrap_or_default() {
        cmd.push(' ');
        cmd.push_str(&a);
    }
    ContainerDetail {
        info,
        command: cmd,
        restart_count: r.restart_count.unwrap_or_default(),
        started_at: state.started_at,
        finished_at: state.finished_at,
        exit_code: state.exit_code,
        health: state.health.and_then(|h| h.status).map(|s| s.to_string()),
        mounts: r
            .mounts
            .unwrap_or_default()
            .into_iter()
            .map(|m| MountInfo {
                source: m.source.unwrap_or_default(),
                destination: m.destination.unwrap_or_default(),
                read_write: m.rw.unwrap_or(true),
            })
            .collect(),
        networks: {
            let mut n: Vec<String> = ns.networks.unwrap_or_default().into_keys().collect();
            n.sort();
            n
        },
    }
}

/// Unix seconds from the leading `YYYY-MM-DDTHH:MM:SS` of an RFC 3339 string (UTC assumed).
fn chrono_like_unix(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = rest
        .split(['.', 'Z', '+'])
        .next()?
        .split(':')
        .map(|p| p.parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    // days from civil (Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// Split one log frame into lines, peeling the Engine's RFC 3339 timestamp prefix.
fn frame_lines(stream: LogStream, bytes: &[u8]) -> Vec<LogLine> {
    String::from_utf8_lossy(bytes)
        .split_inclusive('\n')
        .map(|l| l.trim_end_matches(['\n', '\r']))
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (ts, text) = match l.split_once(' ') {
                Some((ts, rest)) if looks_like_ts(ts) => (Some(ts.to_owned()), rest),
                _ => (None, l),
            };
            LogLine {
                stream,
                timestamp: ts,
                text: text.to_owned(),
            }
        })
        .collect()
}

fn looks_like_ts(s: &str) -> bool {
    s.len() >= 20 && s.as_bytes()[4] == b'-' && s.as_bytes()[10] == b'T'
}

fn output_lines(o: LogOutput) -> Vec<LogLine> {
    match o {
        LogOutput::StdErr { message } => frame_lines(LogStream::Stderr, &message),
        LogOutput::StdOut { message } | LogOutput::Console { message } => {
            frame_lines(LogStream::Stdout, &message)
        }
        LogOutput::StdIn { .. } => vec![],
    }
}

fn cpu_percent(
    prev: &bollard::models::ContainerStatsResponse,
    cur: &bollard::models::ContainerStatsResponse,
) -> Option<f64> {
    let total = |s: &bollard::models::ContainerStatsResponse| {
        s.cpu_stats.as_ref()?.cpu_usage.as_ref()?.total_usage
    };
    let system =
        |s: &bollard::models::ContainerStatsResponse| s.cpu_stats.as_ref()?.system_cpu_usage;
    let cpu_delta = total(cur)?.checked_sub(total(prev)?)? as f64;
    let sys_delta = system(cur)?.checked_sub(system(prev)?)? as f64;
    if sys_delta <= 0.0 {
        return None;
    }
    let cpus = f64::from(cur.cpu_stats.as_ref()?.online_cpus.unwrap_or(1).max(1));
    Some(cpu_delta / sys_delta * cpus * 100.0)
}

#[async_trait::async_trait]
impl DockerPort for BollardDocker {
    async fn ping(&self) -> Result<(), DockerError> {
        self.docker()
            .await?
            .ping()
            .await
            .map(|_| ())
            .map_err(map_err)
    }

    async fn info(&self) -> Result<DockerInfo, DockerError> {
        let d = self.docker().await?;
        let i = d.info().await.map_err(map_err)?;
        let v = d.version().await.map_err(map_err)?;
        Ok(DockerInfo {
            server_version: v.version.unwrap_or_default(),
            api_version: v.api_version.unwrap_or_default(),
            os: i.operating_system.unwrap_or_default(),
            kernel: i.kernel_version.unwrap_or_default(),
            storage_driver: i.driver.unwrap_or_default(),
            root_dir: i.docker_root_dir.unwrap_or_default(),
            containers: i.containers.unwrap_or_default(),
            containers_running: i.containers_running.unwrap_or_default(),
            images: i.images.unwrap_or_default(),
            ncpu: i.ncpu.unwrap_or_default(),
            mem_total: i.mem_total.unwrap_or_default(),
        })
    }

    async fn disk_usage(&self) -> Result<DiskUsage, DockerError> {
        let df = self.docker().await?.df(None).await.map_err(map_err)?;
        macro_rules! bucket {
            ($b:expr) => {
                $b.map(|b| UsageBucket {
                    total_count: b.total_count.unwrap_or_default(),
                    active_count: b.active_count.unwrap_or_default(),
                    size: b.total_size.unwrap_or_default(),
                    reclaimable: b.reclaimable.unwrap_or_default(),
                })
            };
        }
        Ok(DiskUsage {
            images: bucket!(df.image_usage),
            containers: bucket!(df.container_usage),
            volumes: bucket!(df.volume_usage),
            build_cache: bucket!(df.build_cache_usage),
        })
    }

    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, DockerError> {
        let opts = ListContainersOptionsBuilder::new().all(true).build();
        let mut v: Vec<ContainerInfo> = self
            .docker()
            .await?
            .list_containers(Some(opts))
            .await
            .map_err(map_err)?
            .into_iter()
            .map(summary_to_info)
            .collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.created));
        Ok(v)
    }

    async fn container_stats(&self, id: &str) -> Result<Option<ContainerStats>, DockerError> {
        let opts = StatsOptionsBuilder::new()
            .stream(true)
            .one_shot(false)
            .build();
        let d = self.docker().await?;
        let mut s = Box::pin(d.stats(id, Some(opts)));
        let Some(first) = s.next().await else {
            return Ok(None);
        };
        let first = first.map_err(map_err)?;
        let second = match tokio::time::timeout(Duration::from_millis(2500), s.next()).await {
            Ok(Some(Ok(x))) => Some(x),
            _ => None,
        };
        let latest = second.as_ref().unwrap_or(&first);
        let mem = latest.memory_stats.as_ref();
        // Memory in use as `docker stats` reports it: usage minus inactive file cache.
        let cache = mem
            .and_then(|m| m.stats.as_ref())
            .and_then(|s| s.get("inactive_file").or_else(|| s.get("cache")))
            .copied()
            .unwrap_or(0);
        let usage = mem.and_then(|m| m.usage).unwrap_or(0).saturating_sub(cache);
        Ok(Some(ContainerStats {
            cpu_percent: second
                .as_ref()
                .and_then(|c| cpu_percent(&first, c))
                .unwrap_or(0.0),
            mem_usage: usage,
            mem_limit: mem.and_then(|m| m.limit).unwrap_or(0),
        }))
    }

    async fn inspect_container(&self, id: &str) -> Result<ContainerDetail, DockerError> {
        let r = self
            .docker()
            .await?
            .inspect_container(id, None)
            .await
            .map_err(map_err)?;
        let mut d = inspect_to_detail(r, id);
        // `inspect` has no port or status summary; take them from the list entry.
        let filters: HashMap<String, Vec<String>> =
            HashMap::from([("id".to_owned(), vec![d.info.id.clone()])]);
        let opts = ListContainersOptionsBuilder::new()
            .all(true)
            .filters(&filters)
            .build();
        if let Ok(list) = self.docker().await?.list_containers(Some(opts)).await
            && let Some(c) = list.into_iter().next()
        {
            let s = summary_to_info(c);
            d.info.ports = s.ports;
            d.info.status = s.status;
            d.info.created = s.created;
        }
        Ok(d)
    }

    async fn start_container(&self, id: &str) -> Result<(), DockerError> {
        match self.docker().await?.start_container(id, None).await {
            // 304: already started.
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 304, ..
            }) => Ok(()),
            r => r.map_err(map_err),
        }
    }

    async fn stop_container(&self, id: &str, timeout_secs: u32) -> Result<(), DockerError> {
        let opts = StopContainerOptionsBuilder::new()
            .t(i32::try_from(timeout_secs).unwrap_or(10))
            .build();
        match self.docker().await?.stop_container(id, Some(opts)).await {
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 304, ..
            }) => Ok(()),
            r => r.map_err(map_err),
        }
    }

    async fn restart_container(&self, id: &str, timeout_secs: u32) -> Result<(), DockerError> {
        let opts = RestartContainerOptionsBuilder::new()
            .t(i32::try_from(timeout_secs).unwrap_or(10))
            .build();
        self.docker()
            .await?
            .restart_container(id, Some(opts))
            .await
            .map_err(map_err)
    }

    async fn logs(&self, id: &str, tail: u32) -> Result<Vec<LogLine>, DockerError> {
        let opts = LogsOptionsBuilder::new()
            .stdout(true)
            .stderr(true)
            .timestamps(true)
            .tail(&tail.to_string())
            .build();
        let d = self.docker().await?;
        let chunks: Vec<LogOutput> = d
            .logs(id, Some(opts))
            .try_collect()
            .await
            .map_err(map_err)?;
        Ok(chunks.into_iter().flat_map(output_lines).collect())
    }

    fn logs_follow(&self, id: &str, since: i64) -> BoxStream<Result<LogLine, DockerError>> {
        let id = id.to_owned();
        let since = i32::try_from(since.max(0)).unwrap_or(i32::MAX);
        let inner = self.inner.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<LogLine, DockerError>>(256);
        tokio::spawn(async move {
            let d = match inner.docker().await {
                Ok(d) => d.clone(),
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };
            let opts = LogsOptionsBuilder::new()
                .follow(true)
                .stdout(true)
                .stderr(true)
                .timestamps(true)
                .since(since)
                .tail("0")
                .build();
            let mut s = Box::pin(d.logs(&id, Some(opts)));
            loop {
                tokio::select! {
                    () = tx.closed() => return,
                    item = s.next() => match item {
                        None => return,
                        Some(Ok(o)) => {
                            for l in output_lines(o) {
                                if tx.send(Ok(l)).await.is_err() {
                                    return;
                                }
                            }
                        }
                        Some(Err(e)) => {
                            let _ = tx.send(Err(map_err(e))).await;
                            return;
                        }
                    }
                }
            }
        });
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|i| (i, rx))
        }))
    }

    async fn exec_start(&self, id: &str, cols: u16, rows: u16) -> Result<ExecIo, DockerError> {
        let d = self.docker().await?;
        let created = d
            .create_exec(
                id,
                CreateExecOptions {
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(true),
                    env: Some(vec!["TERM=xterm-256color".to_owned()]),
                    cmd: Some(vec![
                        "sh".to_owned(),
                        "-c".to_owned(),
                        SHELL_COMMAND.to_owned(),
                    ]),
                    ..Default::default()
                },
            )
            .await
            .map_err(map_err)?;
        let started = d
            .start_exec(
                &created.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: true,
                    output_capacity: None,
                }),
            )
            .await
            .map_err(map_err)?;
        let StartExecResults::Attached { output, input } = started else {
            return Err(DockerError::Other("exec did not attach".into()));
        };
        // Best effort: the shell starts at the browser's size.
        let _ = d
            .resize_exec(
                &created.id,
                ResizeExecOptions {
                    width: cols,
                    height: rows,
                },
            )
            .await;
        let output = output
            .map(|r| r.map(|o| o.into_bytes().to_vec()).map_err(map_err))
            .boxed();
        Ok(ExecIo {
            exec_id: created.id,
            output,
            input,
        })
    }

    async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> Result<(), DockerError> {
        self.docker()
            .await?
            .resize_exec(
                exec_id,
                ResizeExecOptions {
                    width: cols,
                    height: rows,
                },
            )
            .await
            .map_err(map_err)
    }

    async fn exec_inspect(&self, exec_id: &str) -> Result<ExecState, DockerError> {
        let r = self
            .docker()
            .await?
            .inspect_exec(exec_id)
            .await
            .map_err(map_err)?;
        Ok(ExecState {
            running: r.running.unwrap_or(false),
            exit_code: r.exit_code,
        })
    }

    async fn prune_preview(&self) -> Result<PrunePreviewRaw, DockerError> {
        let d = self.docker().await?;
        let mut out = PrunePreviewRaw::default();

        // Build cache: unused records from `system df` (shape differs by API version; best effort).
        if let Ok(df) = d.df(None).await
            && let Some(items) = df.build_cache_usage.and_then(|b| b.items)
        {
            for it in items {
                let in_use = it
                    .get("InUse")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if in_use {
                    continue;
                }
                let s = |k: &str| it.get(k).and_then(serde_json::Value::as_str).unwrap_or("");
                out.build_cache.push(PruneCandidate {
                    id: s("ID").to_owned(),
                    name: if s("Description").is_empty() {
                        s("ID")
                    } else {
                        s("Description")
                    }
                    .to_owned(),
                    size: it.get("Size").and_then(serde_json::Value::as_u64),
                    labels: BTreeMap::new(),
                });
            }
        }

        // Dangling images only.
        let filters: HashMap<String, Vec<String>> =
            HashMap::from([("dangling".to_owned(), vec!["true".to_owned()])]);
        let imgs = d
            .list_images(Some(
                ListImagesOptionsBuilder::new().filters(&filters).build(),
            ))
            .await
            .map_err(map_err)?;
        for i in imgs {
            out.images.push(PruneCandidate {
                name: i.id.chars().take(19).collect(),
                id: i.id,
                size: u64::try_from(i.size).ok(),
                labels: BTreeMap::new(),
            });
        }

        // Containers and networks come from one container listing.
        let all = d
            .list_containers(Some(ListContainersOptionsBuilder::new().all(true).build()))
            .await
            .map_err(map_err)?;
        let mut used_networks: HashSet<String> = HashSet::new();
        for c in all {
            let info_state = c.state.map(|s| s.to_string()).unwrap_or_default();
            if let Some(ns) = c.network_settings.and_then(|n| n.networks) {
                used_networks.extend(ns.into_keys());
            }
            if !matches!(info_state.as_str(), "running" | "paused" | "restarting") {
                out.containers.push(PruneCandidate {
                    id: c.id.unwrap_or_default(),
                    name: c
                        .names
                        .and_then(|n| n.into_iter().next())
                        .map(|n| clean_name(&n))
                        .unwrap_or_default(),
                    size: None,
                    labels: c.labels.unwrap_or_default().into_iter().collect(),
                });
            }
        }
        let nets = d
            .list_networks(Some(ListNetworksOptionsBuilder::new().build()))
            .await
            .map_err(map_err)?;
        for n in nets {
            let name = n.name.unwrap_or_default();
            if matches!(name.as_str(), "bridge" | "host" | "none") || used_networks.contains(&name)
            {
                continue;
            }
            out.networks.push(PruneCandidate {
                id: n.id.unwrap_or_default(),
                name,
                size: None,
                labels: BTreeMap::new(),
            });
        }
        Ok(out)
    }

    async fn prune(&self, kind: PruneKind) -> Result<PruneOutcome, DockerError> {
        let d = self.docker().await?;
        let (deleted, reclaimed) = match kind {
            PruneKind::BuildCache => {
                let r = d
                    .prune_build(Some(PruneBuildOptionsBuilder::new().build()))
                    .await
                    .map_err(map_err)?;
                (r.caches_deleted.map_or(0, |v| v.len()), r.space_reclaimed)
            }
            PruneKind::Images => {
                let filters: HashMap<String, Vec<String>> =
                    HashMap::from([("dangling".to_owned(), vec!["true".to_owned()])]);
                let r = d
                    .prune_images(Some(
                        PruneImagesOptionsBuilder::new().filters(&filters).build(),
                    ))
                    .await
                    .map_err(map_err)?;
                (r.images_deleted.map_or(0, |v| v.len()), r.space_reclaimed)
            }
            PruneKind::Networks => {
                let r = d.prune_networks(None).await.map_err(map_err)?;
                (r.networks_deleted.map_or(0, |v| v.len()), None)
            }
            PruneKind::Containers => {
                let r = d.prune_containers(None).await.map_err(map_err)?;
                (
                    r.containers_deleted.map_or(0, |v| v.len()),
                    r.space_reclaimed,
                )
            }
        };
        Ok(PruneOutcome {
            deleted: deleted as u64,
            reclaimed_bytes: reclaimed.and_then(|r| u64::try_from(r).ok()).unwrap_or(0),
        })
    }
}
