//! In-process event fan-out for Deployment logs and status changes (01 6.2).

use tokio::sync::broadcast;

use crate::db::repo::deployments::LogChunk;

#[derive(Debug, Clone)]
pub enum HubEvent {
    Log {
        deployment_id: String,
        project_id: String,
        chunk: LogChunk,
    },
    Status {
        deployment_id: String,
        project_id: String,
        status: String,
    },
    /// Terminal status reached; no more events follow for this Deployment.
    Done {
        deployment_id: String,
        project_id: String,
        status: String,
    },
}

impl HubEvent {
    pub fn deployment_id(&self) -> &str {
        match self {
            HubEvent::Log { deployment_id, .. }
            | HubEvent::Status { deployment_id, .. }
            | HubEvent::Done { deployment_id, .. } => deployment_id,
        }
    }

    pub fn project_id(&self) -> &str {
        match self {
            HubEvent::Log { project_id, .. }
            | HubEvent::Status { project_id, .. }
            | HubEvent::Done { project_id, .. } => project_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hub {
    tx: broadcast::Sender<HubEvent>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(4096).0,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.tx.subscribe()
    }

    pub fn publish(&self, event: HubEvent) {
        // No receivers is fine.
        let _ = self.tx.send(event);
    }
}
