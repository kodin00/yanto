//! Scripted [`CommandPort`]: records every command and answers from rules, so tests can
//! assert "exactly these commands ran, in this order" and drive failure paths.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::error::CoreError;
use crate::ports::{
    CancellationToken, CommandOutput, CommandPort, CommandSpec, EXIT_CANCELLED, LogSink, LogStream,
};

/// What a matching command does.
#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub exit_code: i32,
    /// Captured stdout (returned when the spec asked to capture, else streamed).
    pub stdout: String,
    /// Lines written to the sink as stdout (non-captured commands).
    pub lines: Vec<String>,
    pub stderr_lines: Vec<String>,
    /// Block until the cancellation token trips, then report a cancelled command.
    pub hang: bool,
    /// Fail to start (program missing).
    pub spawn_error: bool,
}

impl Reply {
    pub fn ok() -> Self {
        Self::default()
    }

    pub fn stdout(text: impl Into<String>) -> Self {
        Self {
            stdout: text.into(),
            ..Self::default()
        }
    }

    pub fn fail(code: i32, stderr: &str) -> Self {
        Self {
            exit_code: code,
            stderr_lines: vec![stderr.to_owned()],
            ..Self::default()
        }
    }

    pub fn hang() -> Self {
        Self {
            hang: true,
            ..Self::default()
        }
    }
}

/// State of the SSH key file named by `GIT_SSH_COMMAND` at the moment a command ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshKeyProbe {
    pub path: PathBuf,
    pub exists: bool,
    pub mode: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub spec: CommandSpec,
    pub display: String,
    pub ssh_key: Option<SshKeyProbe>,
}

struct Rule {
    pattern: String,
    reply: Reply,
}

#[derive(Default)]
pub struct FakeCommands {
    calls: Mutex<Vec<Recorded>>,
    rules: Mutex<Vec<Rule>>,
}

impl std::fmt::Debug for FakeCommands {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FakeCommands")
    }
}

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

impl FakeCommands {
    /// Sensible defaults for the happy path of a deployment.
    pub fn new() -> Arc<Self> {
        let me = Arc::new(Self::default());
        me.on(
            "config --format json",
            Reply::stdout(r#"{"services":{"app":{"image":"nginx:alpine"}}}"#),
        );
        me.on("config --services", Reply::stdout("app\n"));
        me.on("rev-parse HEAD", Reply::stdout(format!("{SHA}\n")));
        me.on("log -1 --pretty=%s", Reply::stdout("fake commit\n"));
        me.on(
            "image inspect",
            Reply::stdout(r#"["nginx@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]"#),
        );
        me
    }

    /// Answer commands whose `program args...` contains `pattern`. Later rules win.
    pub fn on(&self, pattern: &str, reply: Reply) {
        self.rules.lock().expect("rules").push(Rule {
            pattern: pattern.to_owned(),
            reply,
        });
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .expect("calls")
            .iter()
            .map(|c| c.display.clone())
            .collect()
    }

    pub fn recorded(&self) -> Vec<Recorded> {
        self.calls.lock().expect("calls").clone()
    }

    pub fn clear(&self) {
        self.calls.lock().expect("calls").clear();
    }

    /// Calls containing `needle`.
    pub fn matching(&self, needle: &str) -> Vec<Recorded> {
        self.recorded()
            .into_iter()
            .filter(|c| c.display.contains(needle))
            .collect()
    }
}

fn probe_ssh_key(spec: &CommandSpec) -> Option<SshKeyProbe> {
    let cmd = &spec.env.iter().find(|(k, _)| k == "GIT_SSH_COMMAND")?.1;
    let mut parts = cmd.split_whitespace();
    parts.find(|p| *p == "-i")?;
    let path = PathBuf::from(parts.next()?);
    let meta = std::fs::metadata(&path).ok();
    #[cfg(unix)]
    let mode = meta.as_ref().map(|m| {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o777
    });
    #[cfg(not(unix))]
    let mode = None;
    Some(SshKeyProbe {
        exists: meta.is_some(),
        path,
        mode,
    })
}

#[async_trait]
impl CommandPort for FakeCommands {
    async fn run(
        &self,
        spec: CommandSpec,
        sink: Arc<dyn LogSink>,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, CoreError> {
        let display = spec.display();
        let ssh_key = probe_ssh_key(&spec);
        self.calls.lock().expect("calls").push(Recorded {
            spec: spec.clone(),
            display: display.clone(),
            ssh_key,
        });
        let reply = self
            .rules
            .lock()
            .expect("rules")
            .iter()
            .rev()
            .find(|r| display.contains(&r.pattern))
            .map(|r| r.reply.clone())
            .unwrap_or_default();
        if reply.spawn_error {
            return Err(CoreError::new(
                crate::ErrorKind::Unavailable,
                format!("cannot start {}", spec.program),
            ));
        }
        if reply.hang {
            cancel.cancelled().await;
            return Ok(CommandOutput {
                exit_code: EXIT_CANCELLED,
                cancelled: true,
                ..CommandOutput::default()
            });
        }
        for l in &reply.stderr_lines {
            sink.write(LogStream::Stderr, &format!("{l}\n"));
        }
        if spec.capture_stdout {
            return Ok(CommandOutput {
                exit_code: reply.exit_code,
                stdout: reply.stdout,
                ..CommandOutput::default()
            });
        }
        for l in &reply.lines {
            sink.write(LogStream::Stdout, &format!("{l}\n"));
        }
        Ok(CommandOutput {
            exit_code: reply.exit_code,
            ..CommandOutput::default()
        })
    }
}
