//! The seam to the operating system: command execution, PATH lookup, port and
//! disk probes. Tests substitute [`FakeHost`].

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[cfg(test)]
impl CmdOutput {
    pub fn ok(stdout: &str) -> Self {
        Self {
            success: true,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn fail() -> Self {
        Self {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

pub trait Host {
    /// Effective uid of the running process.
    fn euid(&self) -> u32;
    /// Environment variable lookup.
    fn env(&self, name: &str) -> Option<String>;
    /// Run a program; `Err` means it could not be spawned (e.g. not found).
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CmdOutput>;
    /// Locate an executable on PATH.
    fn which(&self, name: &str) -> Option<PathBuf>;
    /// True when nothing is listening on (and we can bind) `addr:port`.
    fn port_free(&self, addr: IpAddr, port: u16) -> bool;
    /// Free bytes on the filesystem containing `path` (nearest existing ancestor).
    fn free_bytes(&self, path: &Path) -> Option<u64>;
    /// `GET /healthz` returned 200.
    fn healthy(&self, addr: IpAddr, port: u16) -> bool;
    /// Sleep between health polls (fake: no-op).
    fn sleep(&self, d: Duration);
    /// Run and report only success.
    fn succeeds(&self, program: &str, args: &[&str]) -> bool {
        self.run(program, args).is_ok_and(|o| o.success)
    }
}

pub struct RealHost;

impl Host for RealHost {
    fn euid(&self) -> u32 {
        rustix::process::geteuid().as_raw()
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CmdOutput> {
        let out = Command::new(program).args(args).output()?;
        Ok(CmdOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|d| d.join(name))
            .find(|p| is_executable(p))
    }

    fn port_free(&self, addr: IpAddr, port: u16) -> bool {
        TcpListener::bind(SocketAddr::new(addr, port)).is_ok()
    }

    fn free_bytes(&self, path: &Path) -> Option<u64> {
        let mut p = path;
        while !p.exists() {
            p = p.parent()?;
        }
        let st = rustix::fs::statvfs(p).ok()?;
        Some(st.f_bavail.saturating_mul(st.f_frsize))
    }

    fn healthy(&self, addr: IpAddr, port: u16) -> bool {
        let sock = SocketAddr::new(addr, port);
        let Ok(mut s) = TcpStream::connect_timeout(&sock, Duration::from_secs(2)) else {
            return false;
        };
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        if s.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .is_err()
        {
            return false;
        }
        let mut buf = [0u8; 32];
        let n = s.read(&mut buf).unwrap_or(0);
        std::str::from_utf8(&buf[..n])
            .is_ok_and(|t| t.starts_with("HTTP/1.") && t[8..].trim_start().starts_with("200"))
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
pub use fake::FakeHost;

#[cfg(test)]
mod fake {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    /// Scripted host: commands resolve through `responses` keyed by the joined
    /// command line; anything unscripted fails (as if the program were absent).
    pub struct FakeHost {
        pub euid: u32,
        pub env: HashMap<String, String>,
        pub responses: HashMap<String, CmdOutput>,
        pub binaries: HashSet<String>,
        pub busy_ports: HashSet<u16>,
        pub free: Option<u64>,
        pub healthy: bool,
        pub calls: RefCell<Vec<String>>,
    }

    impl Default for FakeHost {
        fn default() -> Self {
            Self {
                euid: 0,
                env: HashMap::new(),
                responses: HashMap::new(),
                binaries: HashSet::new(),
                busy_ports: HashSet::new(),
                free: Some(50 << 30),
                healthy: true,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl FakeHost {
        /// A host where every prerequisite is satisfied.
        pub fn healthy_host() -> Self {
            let mut h = Self::default();
            for (cmd, out) in [
                ("systemctl --version", "systemd 252"),
                ("git --version", "git version 2.39.2"),
                ("docker compose version", "Docker Compose version v2.24.0"),
            ] {
                h.responses.insert(cmd.into(), CmdOutput::ok(out));
            }
            h.responses.insert(
                "docker --host unix:///var/run/docker.sock info --format {{.ServerVersion}}".into(),
                CmdOutput::ok("26.1.0"),
            );
            h.binaries.insert("docker".into());
            h
        }

        pub fn respond(&mut self, cmd: &str, out: CmdOutput) {
            self.responses.insert(cmd.into(), out);
        }
    }

    impl Host for FakeHost {
        fn euid(&self) -> u32 {
            self.euid
        }
        fn env(&self, name: &str) -> Option<String> {
            self.env.get(name).cloned()
        }
        fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CmdOutput> {
            let line = std::iter::once(program)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            self.calls.borrow_mut().push(line.clone());
            self.responses.get(&line).cloned().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, format!("unscripted: {line}"))
            })
        }
        fn which(&self, name: &str) -> Option<PathBuf> {
            self.binaries
                .contains(name)
                .then(|| PathBuf::from("/usr/bin").join(name))
        }
        fn port_free(&self, _addr: IpAddr, port: u16) -> bool {
            !self.busy_ports.contains(&port)
        }
        fn free_bytes(&self, _path: &Path) -> Option<u64> {
            self.free
        }
        fn healthy(&self, _addr: IpAddr, _port: u16) -> bool {
            self.healthy
        }
        fn sleep(&self, _d: Duration) {}
    }
}
