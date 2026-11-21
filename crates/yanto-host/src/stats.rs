//! Host usage via sysinfo (docs/rewrite/04 3.1).
//!
//! CPU percent is measured between two consecutive calls (sysinfo needs a reference sample);
//! the first call waits one minimum interval. Memory, swap, load and uptime are instantaneous.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};
use yanto_core::ports::{DiskStat, HostStatsPort, HostUsage};

const MIN_CPU_INTERVAL: Duration = Duration::from_millis(250);

/// `None` when sysinfo cannot initialise on this platform (usage then reports zeros).
type State = (Option<System>, Option<Instant>);

pub struct SysinfoStats {
    sys: Arc<Mutex<State>>,
}

impl Default for SysinfoStats {
    fn default() -> Self {
        Self::new()
    }
}

impl SysinfoStats {
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_usage())
                .with_memory(MemoryRefreshKind::everything()),
        )
        .ok();
        Self {
            sys: Arc::new(Mutex::new((sys, None))),
        }
    }
}

fn sample(state: &Mutex<State>, disk_paths: &[PathBuf]) -> HostUsage {
    let mut guard = state.lock().expect("sysinfo lock");
    let (sys, last) = &mut *guard;
    let Some(sys) = sys.as_mut() else {
        return HostUsage::default();
    };
    if last.is_none() {
        sys.refresh_cpu_usage();
        *last = Some(Instant::now());
    }
    if let Some(t) = *last {
        let wait = MIN_CPU_INTERVAL.saturating_sub(t.elapsed());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
    sys.refresh_cpu_usage();
    *last = Some(Instant::now());
    sys.refresh_memory();
    let load = System::load_average().unwrap_or(sysinfo::LoadAvg {
        one: 0.0,
        five: 0.0,
        fifteen: 0.0,
    });
    let disks = Disks::new_with_refreshed_list().ok();
    HostUsage {
        cpu_percent: sys.global_cpu_usage(),
        cpu_count: sys.cpus().len(),
        mem_total: sys.total_memory(),
        mem_available: sys.available_memory(),
        swap_total: sys.total_swap(),
        swap_used: sys.used_swap(),
        load_average: [load.one, load.five, load.fifteen],
        uptime_secs: System::uptime().unwrap_or(0),
        disks: disks.map_or_else(Vec::new, |d| disk_stats(&d, disk_paths)),
    }
}

/// Map each path to the disk with the longest matching mount point, deduplicated by mount.
fn disk_stats(disks: &Disks, paths: &[PathBuf]) -> Vec<DiskStat> {
    let mut out: Vec<DiskStat> = Vec::new();
    for p in paths {
        let best = disks
            .iter()
            .filter(|d| Path::new(p).starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len());
        let Some(d) = best else { continue };
        let mount = d.mount_point().to_string_lossy().into_owned();
        let path = p.to_string_lossy().into_owned();
        match out.iter_mut().find(|s| s.mount == mount) {
            Some(s) => {
                if !s.paths.contains(&path) {
                    s.paths.push(path);
                }
            }
            None => out.push(DiskStat {
                mount,
                total_bytes: d.total_space(),
                available_bytes: d.available_space(),
                paths: vec![path],
            }),
        }
    }
    out
}

#[async_trait::async_trait]
impl HostStatsPort for SysinfoStats {
    async fn usage(&self, disk_paths: &[PathBuf]) -> HostUsage {
        // sysinfo does blocking file reads and the CPU sample sleeps; keep it off the async workers.
        let state = self.sys.clone();
        let paths = disk_paths.to_vec();
        tokio::task::spawn_blocking(move || sample(&state, &paths))
            .await
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reports_plausible_numbers() {
        let s = SysinfoStats::new();
        let u = s.usage(&[PathBuf::from("/")]).await;
        assert!(u.mem_total > 0 && u.cpu_count > 0);
        assert!((0.0..=100.0 * u.cpu_count as f32).contains(&u.cpu_percent));
        assert!(u.uptime_secs > 0);
        assert_eq!(u.disks.len(), 1);
        assert!(u.disks[0].total_bytes >= u.disks[0].available_bytes);
    }
}
