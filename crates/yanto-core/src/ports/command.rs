//! Subprocess port (`git`, `docker`, `docker compose`). The real implementation in
//! `yanto-host` spawns without a shell, in its own process group, with a timeout and
//! cancellation (SIGTERM, then SIGKILL after 5 s). Services build the argument vectors
//! and decide what to do with the result; the port only runs one command.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
pub use tokio_util::sync::CancellationToken;

use crate::error::CoreError;

/// Exit code reported for a command stopped by cancellation (as for SIGINT, 128 + 2).
pub const EXIT_CANCELLED: i32 = 130;
/// Exit code reported for a command killed by its timeout (as `timeout(1)`).
pub const EXIT_TIMED_OUT: i32 = 124;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
    System,
}

impl LogStream {
    pub fn as_str(self) -> &'static str {
        match self {
            LogStream::Stdout => "stdout",
            LogStream::Stderr => "stderr",
            LogStream::System => "system",
        }
    }
}

/// Where command output goes while it runs.
pub trait LogSink: Send + Sync {
    fn write(&self, stream: LogStream, text: &str);
}

/// A sink that drops everything (preview/probe commands).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl LogSink for NullSink {
    fn write(&self, _: LogStream, _: &str) {}
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Extra environment on top of the inherited one. Values are never logged.
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
    /// Capture stdout into [`CommandOutput::stdout`] instead of streaming it to the sink.
    pub capture_stdout: bool,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            cwd: None,
            env: Vec::new(),
            timeout: Duration::from_secs(600),
            capture_stdout: false,
        }
    }

    pub fn args_owned(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            cwd: None,
            env: Vec::new(),
            timeout: Duration::from_secs(600),
            capture_stdout: false,
        }
    }

    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, key: &str, value: impl Into<String>) -> Self {
        self.env.push((key.to_owned(), value.into()));
        self
    }

    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    pub fn capture(mut self) -> Self {
        self.capture_stdout = true;
        self
    }

    /// `program arg arg ...` for the log echo (never includes env values).
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub cancelled: bool,
    pub timed_out: bool,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.cancelled && !self.timed_out
    }
}

#[async_trait]
pub trait CommandPort: Send + Sync + 'static {
    /// Run one command to completion. `Err` means the process could not be started
    /// (program missing, bad cwd); a non-zero exit is `Ok` with `exit_code` set.
    async fn run(
        &self,
        spec: CommandSpec,
        sink: Arc<dyn LogSink>,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, CoreError>;
}
