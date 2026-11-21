//! Wall-clock port. Services never call `SystemTime::now()` directly, so tests
//! can move time (session expiry, rate-limit windows, token TTLs).

use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync + 'static {
    /// Unix time in milliseconds, UTC.
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_is_after_2024() {
        assert!(SystemClock.now_ms() > 1_700_000_000_000);
    }
}
