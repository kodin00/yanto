use tracing_subscriber::EnvFilter;
use yanto_core::config::{LogConfig, LogFormat};

/// Install the global subscriber. `RUST_LOG` wins over `log.level`.
pub fn init(cfg: &LogConfig) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&cfg.level))
        .map_err(|e| anyhow::anyhow!("invalid log.level {:?}: {e}", cfg.level))?;
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match cfg.format {
        LogFormat::Json => builder.json().try_init(),
        LogFormat::Pretty => builder.pretty().try_init(),
    }
    .map_err(|e| anyhow::anyhow!("cannot install tracing subscriber: {e}"))
}
