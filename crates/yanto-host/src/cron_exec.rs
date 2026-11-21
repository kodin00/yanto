//! Run a Cron Entry's command as its `run_as_user` (D26, 04 5.3). The command text comes from the
//! DB and is passed to `/bin/sh -c`; the cron file itself never contains it.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, geteuid};
use yanto_core::cron::{DEFAULT_PATH, OUTPUT_TAIL_BYTES};
use yanto_core::ports::{CronExec, ExecOutcome, ExecSpec, UnixUser};
use yanto_core::{CoreError, ErrorKind};

use crate::fsutil::{ensure_private_dir, io_err, try_flock};

#[derive(Debug, Clone)]
pub struct HostCronExec {
    run_dir: PathBuf,
    /// SIGTERM, then SIGKILL after this long (5 s).
    kill_grace: Duration,
    /// The `yanto` binary that provides `exec-as` (defaults to the running executable).
    helper: Option<PathBuf>,
}

impl HostCronExec {
    /// `run_dir` holds the `cron-<id>.lock` files (`/run/yanto`).
    pub fn new(run_dir: impl Into<PathBuf>) -> Self {
        Self {
            run_dir: run_dir.into(),
            kill_grace: Duration::from_secs(5),
            helper: None,
        }
    }

    pub fn with_helper(mut self, exe: impl Into<PathBuf>) -> Self {
        self.helper = Some(exe.into());
        self
    }

    pub fn with_kill_grace(mut self, d: Duration) -> Self {
        self.kill_grace = d;
        self
    }

    fn unavailable(msg: impl Into<String>) -> CoreError {
        CoreError::new(ErrorKind::Unavailable, msg)
    }

    /// `Some(())` when the command must run as another user (we are root and the target differs);
    /// `None` when it is already that user. Anyone else cannot switch.
    fn needs_switch(user: &UnixUser) -> Result<bool, CoreError> {
        let euid = geteuid().as_raw();
        if euid == user.uid {
            return Ok(false);
        }
        if euid != 0 {
            return Err(Self::unavailable(format!(
                "cannot run as {:?}: yanto is not running as root",
                user.name
            )));
        }
        if !cfg!(target_os = "linux") {
            return Err(Self::unavailable(
                "running as another user is only supported on Linux",
            ));
        }
        Ok(true)
    }
}

/// Entry point of the hidden `yanto exec-as` helper: `initgroups`, `setgid`, `setuid`, verify root
/// cannot be regained, then `exec` the program. Returns only on failure. The workspace forbids
/// `unsafe`, so the privilege drop happens in this helper process (nix's safe wrappers) rather than
/// in a `pre_exec` hook.
#[cfg(target_os = "linux")]
pub fn exec_as(name: &str, uid: u32, gid: u32, program: &str, args: &[String]) -> std::io::Error {
    use nix::unistd::{Gid, Uid, getuid, initgroups, setgid, setuid};
    let Ok(cname) = std::ffi::CString::new(name) else {
        return std::io::Error::other("user name contains NUL");
    };
    let steps: [(&str, nix::Result<()>); 3] = [
        ("initgroups", initgroups(&cname, Gid::from_raw(gid))),
        ("setgid", setgid(Gid::from_raw(gid))),
        ("setuid", setuid(Uid::from_raw(uid))),
    ];
    for (what, r) in steps {
        if let Err(e) = r {
            return std::io::Error::other(format!("{what} failed: {e}"));
        }
    }
    if uid != 0 && (getuid().as_raw() != uid || setuid(Uid::from_raw(0)).is_ok()) {
        return std::io::Error::other("privilege drop did not stick");
    }
    Command::new(program).args(args).exec()
}

/// Non-Linux builds (macOS development) cannot switch users.
#[cfg(not(target_os = "linux"))]
pub fn exec_as(_: &str, _: u32, _: u32, _: &str, _: &[String]) -> std::io::Error {
    std::io::Error::other("running as another user is only supported on Linux")
}

/// Keeps the newest `OUTPUT_TAIL_BYTES` of what it is fed.
fn push_tail(buf: &mut Vec<u8>, chunk: &[u8]) {
    buf.extend_from_slice(chunk);
    if buf.len() > OUTPUT_TAIL_BYTES * 2 {
        let cut = buf.len() - OUTPUT_TAIL_BYTES;
        buf.drain(..cut);
    }
}

impl CronExec for HostCronExec {
    fn run(&self, spec: &ExecSpec) -> Result<ExecOutcome, CoreError> {
        // `overlap = skip`: hold the entry's lock for the whole run.
        let _lock = if spec.skip_if_running {
            ensure_private_dir(&self.run_dir)?;
            match try_flock(&self.run_dir.join(format!("cron-{}.lock", spec.entry_id)))? {
                Some(l) => Some(l),
                None => return Ok(ExecOutcome::Skipped),
            }
        } else {
            None
        };

        let switch = Self::needs_switch(&spec.user)?;
        let home = if spec.user.home.is_dir() {
            spec.user.home.clone()
        } else {
            PathBuf::from("/")
        };
        let (reader, writer) = std::io::pipe().map_err(|e| io_err("cannot create a pipe", e))?;
        let writer2 = writer
            .try_clone()
            .map_err(|e| io_err("cannot create a pipe", e))?;
        let mut cmd = if switch {
            // Re-run ourselves as `yanto exec-as`, which drops privileges and execs the shell.
            let exe = match &self.helper {
                Some(p) => p.clone(),
                None => std::env::current_exe()
                    .map_err(|e| io_err("cannot locate the yanto binary", e))?,
            };
            let mut c = Command::new(exe);
            c.arg("exec-as")
                .args(["--uid", &spec.user.uid.to_string()])
                .args(["--gid", &spec.user.gid.to_string()])
                .args(["--name", &spec.user.name])
                .args(["--", "/bin/sh", "-c"])
                .arg(&spec.command);
            c
        } else {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg(&spec.command);
            c
        };
        cmd.env_clear()
            .env("HOME", &spec.user.home)
            .env("LOGNAME", &spec.user.name)
            .env("USER", &spec.user.name)
            .env("SHELL", "/bin/sh")
            .env("PATH", DEFAULT_PATH)
            // Lets `yanto backup` record that it runs from a Cron Entry (04 7.5).
            .env("YANTO_CRON_RUN", &spec.entry_id)
            .current_dir(home)
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(writer2)
            .process_group(0);
        let mut child = cmd
            .spawn()
            .map_err(|e| io_err("cannot start the command", e))?;
        // Close our copies of the write ends, or the reader never sees EOF.
        drop(cmd);

        let out = Arc::new(Mutex::new(Vec::<u8>::new()));
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let sink = Arc::clone(&out);
        let mut reader = reader;
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = reader.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                push_tail(&mut sink.lock().expect("lock"), &chunk[..n]);
            }
            let _ = done_tx.send(());
        });

        let started = Instant::now();
        let pgid = Pid::from_raw(i32::try_from(child.id()).unwrap_or(i32::MAX));
        let mut timed_out = false;
        let mut term_sent_at: Option<Instant> = None;
        let status = loop {
            match child
                .try_wait()
                .map_err(|e| io_err("cannot wait for the command", e))?
            {
                Some(s) => break s,
                None => {
                    let elapsed = started.elapsed();
                    if let Some(limit) = spec.timeout
                        && elapsed >= limit
                        && term_sent_at.is_none()
                    {
                        timed_out = true;
                        let _ = killpg(pgid, Signal::SIGTERM);
                        term_sent_at = Some(Instant::now());
                    }
                    if term_sent_at.is_some_and(|t| t.elapsed() >= self.kill_grace) {
                        let _ = killpg(pgid, Signal::SIGKILL);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        if timed_out {
            // Anything left in the process group (children that ignored SIGTERM) goes too.
            let _ = killpg(pgid, Signal::SIGKILL);
        }
        // Background processes may hold the pipe open; do not wait for them forever.
        let _ = done_rx.recv_timeout(Duration::from_secs(2));
        let mut output = out.lock().expect("lock").clone();
        if output.len() > OUTPUT_TAIL_BYTES {
            output.drain(..output.len() - OUTPUT_TAIL_BYTES);
        }
        Ok(ExecOutcome::Finished {
            exit_code: if timed_out { None } else { status.code() },
            timed_out,
            output,
        })
    }
}
