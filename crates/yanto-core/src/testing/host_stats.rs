//! Scripted [`HostStatsPort`].

use std::path::PathBuf;
use std::sync::Mutex;

use crate::ports::{DiskStat, HostStatsPort, HostUsage};

#[derive(Debug, Default)]
pub struct FakeHostStats {
    usage: Mutex<HostUsage>,
    asked_paths: Mutex<Vec<PathBuf>>,
}

impl FakeHostStats {
    pub fn new() -> Self {
        Self {
            usage: Mutex::new(HostUsage {
                cpu_percent: 12.5,
                cpu_count: 4,
                mem_total: 8 << 30,
                mem_available: 6 << 30,
                swap_total: 1 << 30,
                swap_used: 0,
                load_average: [0.1, 0.2, 0.3],
                uptime_secs: 3600,
                disks: vec![DiskStat {
                    mount: "/".into(),
                    total_bytes: 100 << 30,
                    available_bytes: 40 << 30,
                    paths: vec!["/".into()],
                }],
            }),
            asked_paths: Mutex::new(Vec::new()),
        }
    }

    pub fn asked_paths(&self) -> Vec<PathBuf> {
        self.asked_paths.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl HostStatsPort for FakeHostStats {
    async fn usage(&self, disk_paths: &[PathBuf]) -> HostUsage {
        *self.asked_paths.lock().unwrap() = disk_paths.to_vec();
        self.usage.lock().unwrap().clone()
    }
}
