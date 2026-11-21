//! Host resource usage port (CPU, memory, load, disks). Real implementation: `yanto-host`
//! (sysinfo); the fake returns scripted values.

use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct DiskStat {
    pub mount: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    /// Paths of interest that live on this mount (`/`, data dir, ...).
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct HostUsage {
    pub cpu_percent: f32,
    pub cpu_count: usize,
    pub mem_total: u64,
    pub mem_available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub load_average: [f64; 3],
    pub uptime_secs: u64,
    pub disks: Vec<DiskStat>,
}

#[async_trait::async_trait]
pub trait HostStatsPort: Send + Sync + 'static {
    /// Snapshot of the host; `disk_paths` are mapped to their mount and deduplicated by mount.
    async fn usage(&self, disk_paths: &[PathBuf]) -> HostUsage;
}
