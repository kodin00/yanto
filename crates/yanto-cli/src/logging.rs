use std::fmt::Write as _;
use std::sync::Arc;

use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};
use yanto_core::applog::LogBuffer;
use yanto_core::config::{LogConfig, LogFormat};

/// Feeds every event into the in-memory ring buffer behind the System page's app log (04 3.1).
/// Secrets are masked by the buffer when a line is pushed.
struct BufferLayer(Arc<LogBuffer>);

#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.rest, " {}={value:?}", field.name());
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut f = Fields::default();
        event.record(&mut f);
        let meta = event.metadata();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
        self.0.push(
            ts,
            meta.level().as_str(),
            meta.target(),
            &format!("{}{}", f.message, f.rest),
        );
    }
}

/// Install the global subscriber. `RUST_LOG` wins over `log.level`.
pub fn init(cfg: &LogConfig) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&cfg.level))
        .map_err(|e| anyhow::anyhow!("invalid log.level {:?}: {e}", cfg.level))?;
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(BufferLayer(LogBuffer::global()));
    match cfg.format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .try_init(),
        LogFormat::Pretty => registry
            .with(tracing_subscriber::fmt::layer().pretty())
            .try_init(),
    }
    .map_err(|e| anyhow::anyhow!("cannot install tracing subscriber: {e}"))
}

/// Like [`init`] but writing to stderr: stdout belongs to the MCP protocol (`yanto mcp`).
pub fn init_stderr(cfg: &LogConfig) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&cfg.level))
        .map_err(|e| anyhow::anyhow!("invalid log.level {:?}: {e}", cfg.level))?;
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    match cfg.format {
        LogFormat::Json => builder.json().try_init(),
        LogFormat::Pretty => builder.pretty().try_init(),
    }
    .map_err(|e| anyhow::anyhow!("cannot install tracing subscriber: {e}"))
}
