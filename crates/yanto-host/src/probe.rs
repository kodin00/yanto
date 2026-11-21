//! Listening sockets from `/proc/net/{tcp,tcp6,udp,udp6}` (no `lsof` in a static binary),
//! with a best-effort owner name from `/proc/<pid>/fd` and `comm`.

use std::collections::HashMap;

use async_trait::async_trait;
use yanto_core::CoreError;
use yanto_core::ports::{HostProbePort, Listener};

#[derive(Debug, Default, Clone, Copy)]
pub struct ProcProbe;

#[async_trait]
impl HostProbePort for ProcProbe {
    async fn listeners(&self) -> Result<Vec<Listener>, CoreError> {
        tokio::task::spawn_blocking(read_all)
            .await
            .map_err(|e| CoreError::internal("host probe failed", e))
    }
}

fn read_all() -> Vec<Listener> {
    let mut found: Vec<(u16, &'static str, u64)> = Vec::new();
    for (file, proto) in [
        ("/proc/net/tcp", "tcp"),
        ("/proc/net/tcp6", "tcp"),
        ("/proc/net/udp", "udp"),
        ("/proc/net/udp6", "udp"),
    ] {
        if let Ok(text) = std::fs::read_to_string(file) {
            found.extend(parse_net(&text, proto));
        }
    }
    if found.is_empty() {
        return Vec::new();
    }
    let owners = inode_owners();
    let mut out: Vec<Listener> = found
        .into_iter()
        .map(|(port, protocol, inode)| Listener {
            port,
            protocol: protocol.to_owned(),
            process: owners.get(&inode).cloned(),
        })
        .collect();
    out.sort_by(|a, b| (a.port, &a.protocol).cmp(&(b.port, &b.protocol)));
    out.dedup_by(|a, b| a.port == b.port && a.protocol == b.protocol && a.process == b.process);
    out
}

/// Rows in state LISTEN (`0A`) for TCP, or unconnected (`07`) for UDP.
pub fn parse_net(text: &str, proto: &'static str) -> Vec<(u16, &'static str, u64)> {
    let want = if proto == "tcp" { "0A" } else { "07" };
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 10 || cols[3] != want {
                return None;
            }
            let port = u16::from_str_radix(cols[1].rsplit(':').next()?, 16).ok()?;
            let inode = cols[9].parse().ok()?;
            Some((port, proto, inode))
        })
        .collect()
}

fn inode_owners() -> HashMap<u64, String> {
    let mut map = HashMap::new();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return map;
    };
    for entry in procs.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|c| c.trim().to_owned())
            .unwrap_or_default();
        for fd in fds.flatten() {
            if let Ok(target) = std::fs::read_link(fd.path())
                && let Some(inode) = target
                    .to_str()
                    .and_then(|t| t.strip_prefix("socket:["))
                    .and_then(|t| t.strip_suffix(']'))
                    .and_then(|t| t.parse::<u64>().ok())
            {
                map.entry(inode).or_insert_with(|| comm.clone());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0\n   1: 0100007F:C350 0100007F:1F90 01 00000000:00000000 00:00000000 00000000     0        0 999 1 0000000000000000 100 0 0 10 0\n";

    #[test]
    fn parses_only_listening_rows() {
        let rows = parse_net(TCP, "tcp");
        assert_eq!(rows, vec![(8080, "tcp", 12345)]);
    }

    #[test]
    fn a_bound_listener_is_visible() {
        if !std::path::Path::new("/proc/net/tcp").exists() {
            return; // not Linux
        }
        let sock = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = sock.local_addr().unwrap().port();
        let found = read_all();
        assert!(found.iter().any(|l| l.port == port && l.protocol == "tcp"));
    }
}
