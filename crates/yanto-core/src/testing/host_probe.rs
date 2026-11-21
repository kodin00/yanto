//! Scripted [`HostProbePort`].

use std::sync::Mutex;

use async_trait::async_trait;

use crate::error::CoreError;
use crate::ports::{HostProbePort, Listener};

#[derive(Debug, Default)]
pub struct FakeHostProbe {
    listeners: Mutex<Vec<Listener>>,
}

impl FakeHostProbe {
    pub fn bind(&self, port: u16, protocol: &str, process: Option<&str>) {
        self.listeners.lock().expect("listeners").push(Listener {
            port,
            protocol: protocol.to_owned(),
            process: process.map(str::to_owned),
        });
    }
}

#[async_trait]
impl HostProbePort for FakeHostProbe {
    async fn listeners(&self) -> Result<Vec<Listener>, CoreError> {
        Ok(self.listeners.lock().expect("listeners").clone())
    }
}
