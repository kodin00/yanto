//! A [`LogSink`] that keeps the last few lines, for error messages of quick commands.

use std::collections::VecDeque;
use std::sync::Mutex;

use crate::ports::{LogSink, LogStream};

#[derive(Debug, Default)]
pub struct TailSink {
    lines: Mutex<VecDeque<String>>,
}

const KEEP: usize = 12;

impl TailSink {
    pub fn text(&self) -> String {
        self.lines
            .lock()
            .expect("tail")
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl LogSink for TailSink {
    fn write(&self, _: LogStream, text: &str) {
        let mut q = self.lines.lock().expect("tail");
        for l in text.lines().filter(|l| !l.trim().is_empty()) {
            if q.len() == KEEP {
                q.pop_front();
            }
            q.push_back(l.to_owned());
        }
    }
}
