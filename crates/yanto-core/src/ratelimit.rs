//! In-memory failure limiter (docs/rewrite/03 a.4). State resets on restart (OQ-7).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

/// Sliding-window limiter: at most `max` recorded failures per key per `window`.
/// Only failures are recorded, so successful attempts never consume budget.
#[derive(Debug)]
pub struct FailureLimiter {
    max: usize,
    window_ms: i64,
    state: Mutex<HashMap<String, VecDeque<i64>>>,
}

impl FailureLimiter {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window_ms: i64::try_from(window.as_millis()).unwrap_or(i64::MAX),
            state: Mutex::new(HashMap::new()),
        }
    }

    /// `Err(retry_after_secs)` when `key` is currently locked out.
    pub fn check(&self, key: &str, now_ms: i64) -> Result<(), u64> {
        let mut state = self.state.lock().expect("limiter lock");
        let Some(q) = state.get_mut(key) else {
            return Ok(());
        };
        prune(q, now_ms - self.window_ms);
        if q.len() >= self.max {
            let oldest = q.front().copied().unwrap_or(now_ms);
            let wait_ms = (oldest + self.window_ms - now_ms).max(1000);
            return Err(u64::try_from(wait_ms).unwrap_or(1).div_ceil(1000));
        }
        if q.is_empty() {
            state.remove(key);
        }
        Ok(())
    }

    /// Record a failure. Returns `true` when this failure tripped the limit.
    pub fn record_failure(&self, key: &str, now_ms: i64) -> bool {
        let mut state = self.state.lock().expect("limiter lock");
        let q = state.entry(key.to_owned()).or_default();
        prune(q, now_ms - self.window_ms);
        q.push_back(now_ms);
        // Bound memory under key-spraying: drop idle keys opportunistically.
        let tripped = q.len() == self.max;
        if state.len() > 10_000 {
            let cutoff = now_ms - self.window_ms;
            state.retain(|_, q| q.back().is_some_and(|t| *t > cutoff));
        }
        tripped
    }

    /// Forget a key (for example after a successful login).
    pub fn clear(&self, key: &str) {
        self.state.lock().expect("limiter lock").remove(key);
    }
}

fn prune(q: &mut VecDeque<i64>, cutoff: i64) {
    while q.front().is_some_and(|t| *t <= cutoff) {
        q.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Duration = Duration::from_secs(900);

    #[test]
    fn locks_after_max_failures_and_expires() {
        let l = FailureLimiter::new(3, W);
        assert!(l.check("k", 0).is_ok());
        assert!(!l.record_failure("k", 0));
        assert!(!l.record_failure("k", 1000));
        assert!(l.record_failure("k", 2000));
        let wait = l.check("k", 3000).unwrap_err();
        assert!((895..=900).contains(&wait), "wait = {wait}");
        assert!(l.check("k", 900_001).is_ok());
    }

    #[test]
    fn keys_are_independent_and_clear_resets() {
        let l = FailureLimiter::new(1, W);
        l.record_failure("a", 0);
        assert!(l.check("a", 1).is_err());
        assert!(l.check("b", 1).is_ok());
        l.clear("a");
        assert!(l.check("a", 2).is_ok());
    }
}
