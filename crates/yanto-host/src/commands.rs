//! [`CommandPort`] over `tokio::process`: no shell, own process group, timeout,
//! cancellation (SIGTERM, SIGKILL after 5 s), output streamed line by line.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use yanto_core::CoreError;
use yanto_core::ports::{
    CancellationToken, CommandOutput, CommandPort, CommandSpec, EXIT_CANCELLED, EXIT_TIMED_OUT,
    LogSink, LogStream,
};

const KILL_GRACE: Duration = Duration::from_secs(5);
const READER_GRACE: Duration = Duration::from_secs(2);
const CAPTURE_CAP: usize = 4 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct TokioCommands;

#[async_trait]
impl CommandPort for TokioCommands {
    async fn run(
        &self,
        spec: CommandSpec,
        sink: Arc<dyn LogSink>,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, CoreError> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        if let Some(dir) = &spec.cwd {
            cmd.current_dir(dir);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| {
            CoreError::unavailable(format!("cannot start {}: {}", spec.program, e.kind()), e)
        })?;
        let pid = child.id();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let capture = spec.capture_stdout;
        let out_task = tokio::spawn(pump(stdout, sink.clone(), LogStream::Stdout, capture));
        let err_task = tokio::spawn(pump(stderr, sink, LogStream::Stderr, false));

        let mut cancelled = false;
        let mut timed_out = false;
        let status = tokio::select! {
            s = child.wait() => s,
            () = cancel.cancelled() => {
                cancelled = true;
                terminate(&mut child, pid).await
            }
            () = tokio::time::sleep(spec.timeout) => {
                timed_out = true;
                terminate(&mut child, pid).await
            }
        };
        let stdout_text = join_reader(out_task).await;
        join_reader(err_task).await;
        let exit_code = if cancelled {
            EXIT_CANCELLED
        } else if timed_out {
            EXIT_TIMED_OUT
        } else {
            exit_code_of(status)
        };
        Ok(CommandOutput {
            exit_code,
            stdout: stdout_text,
            cancelled,
            timed_out,
        })
    }
}

fn exit_code_of(status: std::io::Result<std::process::ExitStatus>) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Ok(s) => s
            .code()
            .or_else(|| s.signal().map(|n| 128 + n))
            .unwrap_or(1),
        Err(_) => 1,
    }
}

/// SIGTERM the whole group, wait up to 5 s, then SIGKILL it.
async fn terminate(
    child: &mut tokio::process::Child,
    pid: Option<u32>,
) -> std::io::Result<std::process::ExitStatus> {
    signal_group(pid, nix::sys::signal::Signal::SIGTERM);
    match tokio::time::timeout(KILL_GRACE, child.wait()).await {
        Ok(status) => {
            // Stragglers in the group (children that ignored the leader's exit).
            signal_group(pid, nix::sys::signal::Signal::SIGKILL);
            status
        }
        Err(_) => {
            signal_group(pid, nix::sys::signal::Signal::SIGKILL);
            child.wait().await
        }
    }
}

fn signal_group(pid: Option<u32>, sig: nix::sys::signal::Signal) {
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
        // ESRCH (group already gone) is fine.
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), sig);
    }
}

async fn join_reader(task: tokio::task::JoinHandle<String>) -> String {
    let abort = task.abort_handle();
    match tokio::time::timeout(READER_GRACE, task).await {
        Ok(Ok(text)) => text,
        _ => {
            abort.abort();
            String::new()
        }
    }
}

/// Read a pipe to EOF, sending complete lines to the sink (or collecting them when `capture`).
/// Both `\n` and `\r` end a line, so progress bars do not build one enormous line.
async fn pump<R: AsyncRead + Unpin>(
    mut reader: R,
    sink: Arc<dyn LogSink>,
    stream: LogStream,
    capture: bool,
) -> String {
    let mut captured = String::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 8192];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        pending.extend_from_slice(&buf[..n]);
        if capture {
            if captured.len() < CAPTURE_CAP {
                captured.push_str(&String::from_utf8_lossy(&pending));
            }
            pending.clear();
            continue;
        }
        while let Some(pos) = pending.iter().position(|b| *b == b'\n' || *b == b'\r') {
            let line: Vec<u8> = pending.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]);
            if !text.is_empty() {
                sink.write(stream, &format!("{text}\n"));
            }
        }
        // A pathological line without a terminator must not grow without bound.
        if pending.len() > 64 * 1024 {
            sink.write(stream, &format!("{}\n", String::from_utf8_lossy(&pending)));
            pending.clear();
        }
    }
    if !capture && !pending.is_empty() {
        sink.write(stream, &format!("{}\n", String::from_utf8_lossy(&pending)));
    }
    captured
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Instant;

    use super::*;

    #[derive(Default)]
    struct Collect(Mutex<Vec<(LogStream, String)>>);

    impl LogSink for Collect {
        fn write(&self, s: LogStream, t: &str) {
            self.0.lock().unwrap().push((s, t.to_owned()));
        }
    }

    fn sh(script: &str) -> CommandSpec {
        CommandSpec::new("sh", &["-c", script])
    }

    #[tokio::test]
    async fn streams_lines_and_reports_exit_codes() {
        let sink = Arc::new(Collect::default());
        let out = TokioCommands
            .run(
                sh("echo out; echo err >&2; printf 'a\\rb\\n'; exit 3"),
                sink.clone(),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(out.exit_code, 3);
        assert!(!out.success());
        let lines = sink.0.lock().unwrap().clone();
        assert!(lines.contains(&(LogStream::Stdout, "out\n".into())));
        assert!(lines.contains(&(LogStream::Stderr, "err\n".into())));
        assert!(
            lines.contains(&(LogStream::Stdout, "a\n".into()))
                && lines.contains(&(LogStream::Stdout, "b\n".into()))
        );
    }

    #[tokio::test]
    async fn captures_stdout_instead_of_streaming_it() {
        let sink = Arc::new(Collect::default());
        let out = TokioCommands
            .run(
                sh("echo captured").capture(),
                sink.clone(),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(out.stdout, "captured\n");
        assert!(sink.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn env_cwd_and_missing_program() {
        let dir = tempfile::tempdir().unwrap();
        let out = TokioCommands
            .run(
                sh("echo $FOO; pwd")
                    .env("FOO", "bar")
                    .cwd(dir.path())
                    .capture(),
                Arc::new(Collect::default()),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(out.stdout.starts_with("bar\n"));
        let err = TokioCommands
            .run(
                CommandSpec::new("definitely-not-a-program-yanto", &[]),
                Arc::new(Collect::default()),
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, yanto_core::ErrorKind::Unavailable);
    }

    #[tokio::test]
    async fn timeout_kills_the_command() {
        let started = Instant::now();
        let out = TokioCommands
            .run(
                sh("sleep 30").timeout(Duration::from_millis(200)),
                Arc::new(Collect::default()),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(out.timed_out);
        assert_eq!(out.exit_code, EXIT_TIMED_OUT);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// Cancellation terminates the whole process group, including grandchildren.
    #[tokio::test]
    async fn cancel_kills_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("child.pid");
        let script = format!("sleep 60 & echo $! > {}; wait", pidfile.display());
        let token = CancellationToken::new();
        let t2 = token.clone();
        let started = Instant::now();
        let task = tokio::spawn(async move {
            TokioCommands
                .run(sh(&script), Arc::new(Collect::default()), &t2)
                .await
        });
        for _ in 0..100 {
            if pidfile.exists() && !std::fs::read_to_string(&pidfile).unwrap().trim().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        token.cancel();
        let out = task.await.unwrap().unwrap();
        assert!(out.cancelled && out.exit_code == EXIT_CANCELLED);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "terminated within the grace period"
        );
        let grandchild: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(grandchild), None).is_ok();
        assert!(!alive, "the grandchild must be gone");
    }
}
