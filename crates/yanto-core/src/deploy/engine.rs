//! Shared state of the Deployment machinery: host ports, event hub, limits, and the
//! registry of cancellation tokens for running Deployments.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use super::hub::Hub;
use crate::config::Config;
use crate::ports::{CancellationToken, CommandPort, HostProbePort, RegistryAuthPort};
use crate::service::Ctx;

/// Host side effects used by Projects and Deployments.
#[derive(Clone)]
pub struct HostPorts {
    pub command: Arc<dyn CommandPort>,
    pub probe: Arc<dyn HostProbePort>,
    pub registry_auth: Arc<dyn RegistryAuthPort>,
}

impl std::fmt::Debug for HostPorts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostPorts")
    }
}

pub struct Engine {
    pub ctx: Ctx,
    pub host: HostPorts,
    pub hub: Hub,
    pub data_dir: PathBuf,
    pub max_concurrent: usize,
    pub command_timeout: Duration,
    pub log_max_bytes: i64,
    /// Finished Deployments kept per Project.
    pub retention: i64,
    pub wake: Notify,
    cancels: Mutex<HashMap<String, CancellationToken>>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Engine")
    }
}

impl Engine {
    pub fn new(ctx: Ctx, host: HostPorts, config: &Config) -> Arc<Self> {
        Arc::new(Self {
            ctx,
            host,
            hub: Hub::new(),
            data_dir: config.paths.data_dir.clone(),
            max_concurrent: config.deploy.max_concurrent.max(1) as usize,
            command_timeout: config
                .command_timeout()
                .unwrap_or(Duration::from_secs(3600)),
            log_max_bytes: i64::try_from(config.deploy.log_max_bytes).unwrap_or(500_000),
            retention: 50,
            wake: Notify::new(),
            cancels: Mutex::new(HashMap::new()),
        })
    }

    /// Register the token a running Deployment observes.
    pub fn register_cancel(&self, deployment_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        self.cancels
            .lock()
            .expect("cancels")
            .insert(deployment_id.to_owned(), token.clone());
        token
    }

    pub fn unregister_cancel(&self, deployment_id: &str) {
        self.cancels.lock().expect("cancels").remove(deployment_id);
    }

    /// Trip the token of a running Deployment. `false` when it is not running in this process.
    pub fn cancel(&self, deployment_id: &str) -> bool {
        match self.cancels.lock().expect("cancels").get(deployment_id) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    pub fn cancel_all(&self) {
        for t in self.cancels.lock().expect("cancels").values() {
            t.cancel();
        }
    }

    pub fn wake_worker(&self) {
        self.wake.notify_one();
    }
}
