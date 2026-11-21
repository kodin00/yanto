//! In-memory ring buffer of the last application log lines (docs/rewrite/04 3.1, 01 section 9):
//! what the System page shows and follows. A `tracing` layer in the binary pushes into it; older
//! history is `journalctl -u yanto`. Secrets are masked when a line is pushed, never on read.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tokio::sync::broadcast;

/// Lines kept (04 3.1).
pub const CAPACITY: usize = 500;
const MAX_LINE: usize = 4096;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppLogLine {
    pub seq: u64,
    /// Unix milliseconds.
    pub ts: i64,
    pub level: String,
    pub target: String,
    pub text: String,
}

#[derive(Debug)]
pub struct LogBuffer {
    lines: Mutex<VecDeque<AppLogLine>>,
    next_seq: Mutex<u64>,
    tx: broadcast::Sender<AppLogLine>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: Mutex::new(VecDeque::with_capacity(capacity)),
            next_seq: Mutex::new(1),
            tx: broadcast::channel(256).0,
            capacity: capacity.max(1),
        }
    }

    /// The process-wide buffer the `tracing` layer feeds.
    pub fn global() -> Arc<LogBuffer> {
        static G: OnceLock<Arc<LogBuffer>> = OnceLock::new();
        G.get_or_init(|| Arc::new(LogBuffer::new(CAPACITY))).clone()
    }

    /// Mask secrets in `text`, then append the line and wake followers.
    pub fn push(&self, ts: i64, level: &str, target: &str, text: &str) {
        let mut text = redact(text);
        if text.len() > MAX_LINE {
            let mut cut = MAX_LINE;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push_str("...");
        }
        let mut lines = self.lines.lock().expect("log buffer");
        let mut seq = self.next_seq.lock().expect("log seq");
        let line = AppLogLine {
            seq: *seq,
            ts,
            level: level.to_owned(),
            target: target.to_owned(),
            text,
        };
        *seq += 1;
        if lines.len() == self.capacity {
            lines.pop_front();
        }
        lines.push_back(line.clone());
        // No receiver is fine: nobody is following.
        let _ = self.tx.send(line);
    }

    /// The newest `n` lines, oldest first.
    pub fn tail(&self, n: usize) -> Vec<AppLogLine> {
        let lines = self.lines.lock().expect("log buffer");
        let skip = lines.len().saturating_sub(n);
        lines.iter().skip(skip).cloned().collect()
    }

    /// Backlog plus a receiver that gets every line pushed after it, without a gap or a repeat.
    pub fn follow(&self, backlog: usize) -> (Vec<AppLogLine>, broadcast::Receiver<AppLogLine>) {
        let lines = self.lines.lock().expect("log buffer");
        let rx = self.tx.subscribe();
        let skip = lines.len().saturating_sub(backlog);
        (lines.iter().skip(skip).cloned().collect(), rx)
    }
}

/// Words that carry a secret: our token formats, sealed values, bearer credentials, and the value
/// of `password=`/`secret=`/`token=`/`authorization:` style fields. Emission-time redaction is a
/// backstop; code is not supposed to log secrets in the first place.
pub fn redact(text: &str) -> String {
    const MASK: &str = "[redacted]";
    const PREFIXES: [&str; 5] = ["yat_", "ydp_", "enc:v1:", "ymcp_", "-----BEGIN"];
    const KEYS: [&str; 7] = [
        "password",
        "passwd",
        "secret",
        "token",
        "authorization",
        "api_key",
        "apikey",
    ];
    let mut out = String::with_capacity(text.len());
    let mut mask_next = false;
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let lower = word.to_ascii_lowercase();
        let bare = lower.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':');
        if mask_next {
            mask_next = false;
            if lower == "bearer" || lower == "basic" {
                // `Authorization: Bearer <credential>`: the scheme stays, the credential goes.
                out.push_str(word);
                mask_next = true;
                continue;
            }
            if !word.is_empty() {
                out.push_str(MASK);
                continue;
            }
        }
        if PREFIXES
            .iter()
            .any(|p| bare.contains(&p.to_ascii_lowercase()))
        {
            out.push_str(MASK);
            continue;
        }
        // `key=value`, `key:value`, `"key":"value"`
        if let Some(pos) = word.find(['=', ':']) {
            let key = lower[..pos].trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
            if KEYS.contains(&key) && pos + 1 < word.len() {
                out.push_str(&word[..=pos]);
                out.push_str(MASK);
                continue;
            }
            if KEYS.contains(&key) && pos + 1 == word.len() {
                out.push_str(word);
                mask_next = true;
                continue;
            }
        }
        if lower == "bearer" || lower == "basic" {
            out.push_str(word);
            mask_next = true;
            continue;
        }
        out.push_str(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_lines_and_numbers_them() {
        let b = LogBuffer::new(3);
        for i in 0..5 {
            b.push(i, "INFO", "t", &format!("line {i}"));
        }
        let t = b.tail(10);
        assert_eq!(
            t.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["line 2", "line 3", "line 4"]
        );
        assert_eq!(t[0].seq, 3);
        assert_eq!(b.tail(1)[0].text, "line 4");
    }

    #[tokio::test]
    async fn follow_has_no_gap_between_backlog_and_live_lines() {
        let b = LogBuffer::new(10);
        b.push(0, "INFO", "t", "old");
        let (backlog, mut rx) = b.follow(5);
        b.push(1, "INFO", "t", "new");
        assert_eq!(backlog.len(), 1);
        assert_eq!(rx.recv().await.unwrap().text, "new");
    }

    #[test]
    fn secrets_are_masked_when_pushed() {
        let b = LogBuffer::new(10);
        b.push(
            0,
            "INFO",
            "t",
            "login ok token=abc123 Authorization: Bearer s3cr3t key yat_1_abcdef password: hunter2 enc:v1:ZZZ done",
        );
        let t = &b.tail(1)[0].text;
        for leaked in ["abc123", "s3cr3t", "yat_1_abcdef", "hunter2", "enc:v1:ZZZ"] {
            assert!(!t.contains(leaked), "{leaked} leaked: {t}");
        }
        assert!(t.starts_with("login ok") && t.ends_with("done"), "{t}");
    }

    #[test]
    fn long_lines_are_cut_on_a_char_boundary() {
        let b = LogBuffer::new(2);
        b.push(0, "INFO", "t", &"é".repeat(5000));
        let t = &b.tail(1)[0].text;
        assert!(t.len() <= MAX_LINE + 3 && t.ends_with("..."));
    }
}
