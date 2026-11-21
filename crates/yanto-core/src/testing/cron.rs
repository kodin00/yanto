//! Fakes for [`CronPort`] and [`CronExec`]: record calls, return scripted results.

use std::collections::VecDeque;
use std::sync::Mutex;

use crate::error::CoreError;
use crate::ports::{CronExec, CronPort, CronSource, ExecOutcome, ExecSpec, ManagedFile};

#[derive(Debug)]
pub struct FakeCron {
    pub binary: String,
    state: Mutex<CronState>,
}

#[derive(Debug)]
struct CronState {
    managed: ManagedFile,
    writes: Vec<String>,
    fail_next: Option<CoreError>,
    system: Vec<CronSource>,
    active: Option<bool>,
}

impl Default for FakeCron {
    fn default() -> Self {
        Self {
            binary: "/usr/local/bin/yanto".into(),
            state: Mutex::new(CronState {
                managed: ManagedFile::Missing,
                writes: Vec::new(),
                fail_next: None,
                system: Vec::new(),
                active: Some(true),
            }),
        }
    }
}

impl FakeCron {
    /// Every content passed to `write_managed`, oldest first.
    pub fn writes(&self) -> Vec<String> {
        self.state.lock().expect("lock").writes.clone()
    }

    /// The current "file" content, if present.
    pub fn content(&self) -> Option<String> {
        match &self.state.lock().expect("lock").managed {
            ManagedFile::Present(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// Simulate someone editing the file by hand.
    pub fn set_managed(&self, m: ManagedFile) {
        self.state.lock().expect("lock").managed = m;
    }

    /// Make the next `write_managed` fail with `err` (nothing is written).
    pub fn fail_next(&self, err: CoreError) {
        self.state.lock().expect("lock").fail_next = Some(err);
    }

    pub fn set_system_sources(&self, s: Vec<CronSource>) {
        self.state.lock().expect("lock").system = s;
    }

    pub fn set_service_active(&self, a: Option<bool>) {
        self.state.lock().expect("lock").active = a;
    }
}

impl CronPort for FakeCron {
    fn yanto_binary(&self) -> Result<String, CoreError> {
        Ok(self.binary.clone())
    }

    fn read_managed(&self) -> Result<ManagedFile, CoreError> {
        Ok(self.state.lock().expect("lock").managed.clone())
    }

    fn write_managed(&self, content: &str, replace_unsafe: bool) -> Result<(), CoreError> {
        let mut s = self.state.lock().expect("lock");
        if let Some(e) = s.fail_next.take() {
            return Err(e);
        }
        if let ManagedFile::Unsafe(why) = &s.managed
            && !replace_unsafe
        {
            return Err(CoreError::conflict(format!("refusing to write: {why}")));
        }
        s.writes.push(content.to_owned());
        s.managed = ManagedFile::Present(content.to_owned());
        Ok(())
    }

    fn system_sources(&self) -> Result<Vec<CronSource>, CoreError> {
        Ok(self.state.lock().expect("lock").system.clone())
    }

    fn service_active(&self) -> Option<bool> {
        self.state.lock().expect("lock").active
    }
}

#[derive(Debug, Default)]
pub struct FakeCronExec {
    script: Mutex<VecDeque<Result<ExecOutcome, CoreError>>>,
    calls: Mutex<Vec<ExecSpec>>,
}

impl FakeCronExec {
    /// Queue the result of the next `run`. Without one, a run exits 0 with output `ok`.
    pub fn push(&self, r: Result<ExecOutcome, CoreError>) {
        self.script.lock().expect("lock").push_back(r);
    }

    pub fn calls(&self) -> Vec<ExecSpec> {
        self.calls.lock().expect("lock").clone()
    }
}

impl CronExec for FakeCronExec {
    fn run(&self, spec: &ExecSpec) -> Result<ExecOutcome, CoreError> {
        self.calls.lock().expect("lock").push(spec.clone());
        self.script
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or(Ok(ExecOutcome::Finished {
                exit_code: Some(0),
                timed_out: false,
                output: b"ok".to_vec(),
            }))
    }
}
