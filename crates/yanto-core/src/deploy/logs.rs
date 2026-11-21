//! Deployment log writer: batches output every 250 ms or 64 KiB into `deployment_logs`,
//! caps the stored size, redacts known secrets, and publishes to the [`super::hub::Hub`].

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use super::engine::Engine;
use super::hub::HubEvent;
use crate::db::repo::deployments::{self, LogChunk};
use crate::ports::{CommandLogStream, LogSink};

const FLUSH_EVERY: Duration = Duration::from_millis(250);
const CHUNK_MAX: usize = 64 * 1024;

/// Exact-match secret scrubber (best effort, 04 1.5).
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut secrets: Vec<String> = secrets.into_iter().filter(|s| s.len() >= 4).collect();
        // Longest first so a secret containing another is replaced whole.
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Self { secrets }
    }

    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for s in &self.secrets {
            if out.contains(s.as_str()) {
                out = out.replace(s.as_str(), "***");
            }
        }
        out
    }
}

enum Msg {
    Text(CommandLogStream, String),
    Close(oneshot::Sender<()>),
}

pub struct DeployLogger {
    tx: mpsc::UnboundedSender<Msg>,
    redactor: Redactor,
}

impl LogSink for DeployLogger {
    fn write(&self, stream: CommandLogStream, text: &str) {
        let _ = self.tx.send(Msg::Text(stream, self.redactor.apply(text)));
    }
}

impl DeployLogger {
    pub fn system(&self, line: &str) {
        self.write(CommandLogStream::System, &format!("{line}\n"));
    }
}

pub struct LogHandle {
    pub sink: Arc<DeployLogger>,
    done: tokio::task::JoinHandle<()>,
}

impl LogHandle {
    /// Flush everything and stop the writer.
    pub async fn close(self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        if self.sink.tx.send(Msg::Close(ack_tx)).is_ok() {
            let _ = ack_rx.await;
        }
        let _ = self.done.await;
    }
}

pub fn spawn(
    engine: Arc<Engine>,
    deployment_id: String,
    project_id: String,
    redactor: Redactor,
) -> LogHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let sink = Arc::new(DeployLogger { tx, redactor });
    let done = tokio::spawn(writer(engine, deployment_id, project_id, rx));
    LogHandle { sink, done }
}

async fn writer(
    engine: Arc<Engine>,
    deployment_id: String,
    project_id: String,
    mut rx: mpsc::UnboundedReceiver<Msg>,
) {
    let mut seq = match engine.ctx.db.reader().acquire().await {
        Ok(mut c) => deployments::max_seq(&mut c, &deployment_id)
            .await
            .unwrap_or(0),
        Err(_) => 0,
    };
    let mut buf: Vec<(CommandLogStream, String)> = Vec::new();
    let mut size = 0usize;
    let mut tick = tokio::time::interval(FLUSH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(Msg::Text(stream, text)) => {
                    size += text.len();
                    match buf.last_mut() {
                        Some((s, t)) if *s == stream => t.push_str(&text),
                        _ => buf.push((stream, text)),
                    }
                    if size >= CHUNK_MAX {
                        flush(&engine, &deployment_id, &project_id, &mut seq, &mut buf).await;
                        size = 0;
                    }
                }
                Some(Msg::Close(ack)) => {
                    flush(&engine, &deployment_id, &project_id, &mut seq, &mut buf).await;
                    let _ = ack.send(());
                    return;
                }
                None => {
                    flush(&engine, &deployment_id, &project_id, &mut seq, &mut buf).await;
                    return;
                }
            },
            _ = tick.tick() => {
                if !buf.is_empty() {
                    flush(&engine, &deployment_id, &project_id, &mut seq, &mut buf).await;
                    size = 0;
                }
            }
        }
    }
}

/// Split `text` into pieces of at most [`CHUNK_MAX`] bytes on character boundaries.
fn split_chunks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while rest.len() > CHUNK_MAX {
        let mut cut = CHUNK_MAX;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        out.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

async fn flush(
    engine: &Engine,
    deployment_id: &str,
    project_id: &str,
    seq: &mut i64,
    buf: &mut Vec<(CommandLogStream, String)>,
) {
    if buf.is_empty() {
        return;
    }
    let ts = engine.ctx.now();
    let mut chunks = Vec::new();
    for (stream, text) in buf.drain(..) {
        for piece in split_chunks(&text) {
            *seq += 1;
            chunks.push(LogChunk {
                seq: *seq,
                ts,
                stream: stream.as_str().to_owned(),
                data: piece.to_owned(),
            });
        }
    }
    let result = async {
        let mut tx = engine.ctx.db.begin_write().await?;
        let mut bytes = 0i64;
        for c in &chunks {
            deployments::append_log(&mut tx, deployment_id, c).await?;
            bytes += i64::try_from(c.data.len()).unwrap_or(0);
        }
        let total = deployments::add_log_bytes(&mut tx, deployment_id, bytes).await?;
        if total > engine.log_max_bytes {
            deployments::truncate_logs(&mut tx, deployment_id, engine.log_max_bytes, ts).await?;
        }
        tx.commit().await.map_err(crate::db::repo::db_err)
    }
    .await;
    if let Err(err) = result {
        tracing::warn!(error = %err, deployment = deployment_id, "could not store deployment log chunk");
    }
    for chunk in chunks {
        engine.hub.publish(HubEvent::Log {
            deployment_id: deployment_id.to_owned(),
            project_id: project_id.to_owned(),
            chunk,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_replaces_known_values_only() {
        let r = Redactor::new([
            "supersecret".to_owned(),
            "ab".to_owned(),
            "supersecret2".to_owned(),
        ]);
        assert_eq!(
            r.apply("token=supersecret2 x supersecret"),
            "token=*** x ***"
        );
        assert_eq!(
            r.apply("ab stays"),
            "ab stays",
            "short values are not scrubbed"
        );
    }

    #[test]
    fn chunks_respect_char_boundaries() {
        let text = "é".repeat(CHUNK_MAX);
        let parts = split_chunks(&text);
        assert!(parts.iter().all(|p| p.len() <= CHUNK_MAX));
        assert_eq!(parts.concat(), text);
    }
}
