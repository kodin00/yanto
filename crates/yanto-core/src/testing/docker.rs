//! Scripted [`DockerPort`] for tests: records every call, serves canned containers and logs,
//! and can fail the next call (`FakeDocker::fail_next`).

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::AsyncWrite;

use crate::ports::{
    BoxStream, ContainerDetail, ContainerInfo, ContainerStats, DiskUsage, DockerError, DockerInfo,
    DockerPort, ExecIo, ExecState, LogLine, PruneCandidate, PruneKind, PruneOutcome,
    PrunePreviewRaw,
};

#[derive(Default)]
struct State {
    containers: Vec<ContainerInfo>,
    stats: HashMap<String, ContainerStats>,
    calls: Vec<String>,
    fail_next: Option<DockerError>,
    logs: Vec<LogLine>,
    follow_logs: Vec<LogLine>,
    exec_output: Vec<Vec<u8>>,
    exec_input: Arc<Mutex<Vec<u8>>>,
    exec_input_closed: Arc<Mutex<bool>>,
    resizes: Vec<(u16, u16)>,
    exec_running: bool,
    exec_exit: Option<i64>,
    preview: PrunePreviewRaw,
    prune_outcome: PruneOutcome,
    list_calls: usize,
}

#[derive(Default)]
pub struct FakeDocker {
    state: Mutex<State>,
}

impl std::fmt::Debug for FakeDocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeDocker").finish_non_exhaustive()
    }
}

struct SharedWriter {
    buf: Arc<Mutex<Vec<u8>>>,
    closed: Arc<Mutex<bool>>,
}

impl AsyncWrite for SharedWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.buf.lock().unwrap().extend_from_slice(data);
        Poll::Ready(Ok(data.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        *self.closed.lock().unwrap() = true;
        Poll::Ready(Ok(()))
    }
}

impl Drop for SharedWriter {
    fn drop(&mut self) {
        *self.closed.lock().unwrap() = true;
    }
}

impl FakeDocker {
    pub fn new() -> Self {
        Self::default()
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Add a container; `id` is derived from `name` padded to 64 hex-looking characters.
    pub fn add_container(&self, name: &str, state: &str, labels: &[(&str, &str)]) -> String {
        let id = format!(
            "{:0<64}",
            name.bytes().map(|b| format!("{b:02x}")).collect::<String>()
        );
        let id = id[..64].to_owned();
        let mut s = self.st();
        let created = 1_767_225_600 + i64::try_from(s.containers.len()).unwrap();
        s.containers.push(ContainerInfo {
            id: id.clone(),
            name: name.to_owned(),
            image: "alpine:3".into(),
            state: state.to_owned(),
            status: format!("{state} (fake)"),
            created,
            ports: vec![],
            labels: labels
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        });
        id
    }

    pub fn set_stats(&self, id: &str, stats: ContainerStats) {
        self.st().stats.insert(id.to_owned(), stats);
    }

    pub fn set_logs(&self, lines: Vec<LogLine>) {
        self.st().logs = lines;
    }

    pub fn set_follow_logs(&self, lines: Vec<LogLine>) {
        self.st().follow_logs = lines;
    }

    pub fn set_exec_output(&self, chunks: Vec<Vec<u8>>) {
        self.st().exec_output = chunks;
    }

    /// What `inspect_exec` reports after the session closes.
    pub fn set_exec_state(&self, running: bool, exit: Option<i64>) {
        let mut s = self.st();
        s.exec_running = running;
        s.exec_exit = exit;
    }

    pub fn set_preview(&self, preview: PrunePreviewRaw) {
        self.st().preview = preview;
    }

    pub fn set_prune_outcome(&self, outcome: PruneOutcome) {
        self.st().prune_outcome = outcome;
    }

    pub fn fail_next(&self, err: DockerError) {
        self.st().fail_next = Some(err);
    }

    pub fn calls(&self) -> Vec<String> {
        self.st().calls.clone()
    }

    pub fn list_calls(&self) -> usize {
        self.st().list_calls
    }

    pub fn exec_input(&self) -> Vec<u8> {
        self.st().exec_input.lock().unwrap().clone()
    }

    pub fn exec_input_closed(&self) -> bool {
        *self.st().exec_input_closed.lock().unwrap()
    }

    pub fn resizes(&self) -> Vec<(u16, u16)> {
        self.st().resizes.clone()
    }

    pub fn candidate(id: &str, name: &str, size: Option<u64>) -> PruneCandidate {
        PruneCandidate {
            id: id.into(),
            name: name.into(),
            size,
            labels: BTreeMap::new(),
        }
    }

    fn enter(&self, call: String) -> Result<(), DockerError> {
        let mut s = self.st();
        s.calls.push(call);
        match s.fail_next.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn find(&self, id: &str) -> Result<ContainerInfo, DockerError> {
        self.st()
            .containers
            .iter()
            .find(|c| c.id == id || c.name == id)
            .cloned()
            .ok_or(DockerError::NotFound)
    }

    fn set_state(&self, id: &str, state: &str) {
        if let Some(c) = self.st().containers.iter_mut().find(|c| c.id == id) {
            c.state = state.into();
        }
    }
}

#[async_trait::async_trait]
impl DockerPort for FakeDocker {
    async fn ping(&self) -> Result<(), DockerError> {
        self.enter("ping".into())
    }

    async fn info(&self) -> Result<DockerInfo, DockerError> {
        self.enter("info".into())?;
        let s = self.st();
        Ok(DockerInfo {
            server_version: "0.0-fake".into(),
            api_version: "1.0".into(),
            os: "FakeOS".into(),
            kernel: "0".into(),
            storage_driver: "overlay2".into(),
            root_dir: "/var/lib/docker".into(),
            containers: i64::try_from(s.containers.len()).unwrap(),
            containers_running: i64::try_from(
                s.containers.iter().filter(|c| c.is_running()).count(),
            )
            .unwrap(),
            images: 1,
            ncpu: 2,
            mem_total: 1 << 30,
        })
    }

    async fn disk_usage(&self) -> Result<DiskUsage, DockerError> {
        self.enter("disk_usage".into())?;
        Ok(DiskUsage::default())
    }

    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, DockerError> {
        self.enter("list_containers".into())?;
        let mut s = self.st();
        s.list_calls += 1;
        let mut v = s.containers.clone();
        v.sort_by_key(|c| std::cmp::Reverse(c.created));
        Ok(v)
    }

    async fn container_stats(&self, id: &str) -> Result<Option<ContainerStats>, DockerError> {
        self.enter(format!("stats {id}"))?;
        Ok(self.st().stats.get(id).copied())
    }

    async fn inspect_container(&self, id: &str) -> Result<ContainerDetail, DockerError> {
        self.enter(format!("inspect {id}"))?;
        let info = self.find(id)?;
        Ok(ContainerDetail {
            info,
            command: "sleep 3600".into(),
            restart_count: 0,
            started_at: None,
            finished_at: None,
            exit_code: None,
            health: None,
            mounts: vec![],
            networks: vec!["bridge".into()],
        })
    }

    async fn start_container(&self, id: &str) -> Result<(), DockerError> {
        self.enter(format!("start {id}"))?;
        let c = self.find(id)?;
        self.set_state(&c.id, "running");
        Ok(())
    }

    async fn stop_container(&self, id: &str, timeout_secs: u32) -> Result<(), DockerError> {
        self.enter(format!("stop {id} t={timeout_secs}"))?;
        let c = self.find(id)?;
        self.set_state(&c.id, "exited");
        Ok(())
    }

    async fn restart_container(&self, id: &str, timeout_secs: u32) -> Result<(), DockerError> {
        self.enter(format!("restart {id} t={timeout_secs}"))?;
        let c = self.find(id)?;
        self.set_state(&c.id, "running");
        Ok(())
    }

    async fn logs(&self, id: &str, tail: u32) -> Result<Vec<LogLine>, DockerError> {
        self.enter(format!("logs {id} tail={tail}"))?;
        self.find(id)?;
        let s = self.st();
        let n = s.logs.len().saturating_sub(tail as usize);
        Ok(s.logs[n..].to_vec())
    }

    fn logs_follow(&self, id: &str, since: i64) -> BoxStream<Result<LogLine, DockerError>> {
        let res = self
            .enter(format!("logs_follow {id} since={since}"))
            .and_then(|()| self.find(id));
        let lines: Vec<Result<LogLine, DockerError>> = match res {
            Ok(_) => self.st().follow_logs.iter().cloned().map(Ok).collect(),
            Err(e) => vec![Err(e)],
        };
        Box::pin(futures_util::stream::iter(lines))
    }

    async fn exec_start(&self, id: &str, cols: u16, rows: u16) -> Result<ExecIo, DockerError> {
        self.enter(format!("exec_start {id} {cols}x{rows}"))?;
        let c = self.find(id)?;
        if !c.is_running() {
            return Err(DockerError::Conflict("container is not running".into()));
        }
        let s = self.st();
        *s.exec_input_closed.lock().unwrap() = false;
        let chunks: Vec<Result<Vec<u8>, DockerError>> =
            s.exec_output.iter().cloned().map(Ok).collect();
        Ok(ExecIo {
            exec_id: "exec_fake".into(),
            output: Box::pin(futures_util::StreamExt::chain(
                futures_util::stream::iter(chunks),
                futures_util::stream::pending(),
            )),
            input: Box::pin(SharedWriter {
                buf: s.exec_input.clone(),
                closed: s.exec_input_closed.clone(),
            }),
        })
    }

    async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> Result<(), DockerError> {
        self.enter(format!("exec_resize {exec_id} {cols}x{rows}"))?;
        self.st().resizes.push((cols, rows));
        Ok(())
    }

    async fn exec_inspect(&self, exec_id: &str) -> Result<ExecState, DockerError> {
        self.enter(format!("exec_inspect {exec_id}"))?;
        let s = self.st();
        Ok(ExecState {
            running: s.exec_running,
            exit_code: s.exec_exit,
        })
    }

    async fn prune_preview(&self) -> Result<PrunePreviewRaw, DockerError> {
        self.enter("prune_preview".into())?;
        Ok(self.st().preview.clone())
    }

    async fn prune(&self, kind: PruneKind) -> Result<PruneOutcome, DockerError> {
        self.enter(format!("prune {}", kind.as_str()))?;
        Ok(self.st().prune_outcome)
    }
}
