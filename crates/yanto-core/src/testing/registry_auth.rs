//! In-memory [`RegistryAuthPort`]: tracks which scopes are live so tests can assert the
//! temp `DOCKER_CONFIG` is created for a command and removed afterwards.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::CoreError;
use crate::ports::{AuthEntry, RegistryAuthPort};

#[derive(Debug, Default)]
pub struct FakeRegistryAuth {
    live: Mutex<BTreeSet<String>>,
    /// Every create call: scope, `registry_key:username` pairs (never the secret).
    created: Mutex<Vec<(String, Vec<String>)>>,
    /// Secrets seen at create time, for tests that check redaction.
    secrets: Mutex<Vec<String>>,
}

impl FakeRegistryAuth {
    pub fn live(&self) -> Vec<String> {
        self.live.lock().expect("live").iter().cloned().collect()
    }

    pub fn created(&self) -> Vec<(String, Vec<String>)> {
        self.created.lock().expect("created").clone()
    }

    pub fn secrets_seen(&self) -> Vec<String> {
        self.secrets.lock().expect("secrets").clone()
    }
}

impl RegistryAuthPort for FakeRegistryAuth {
    fn create(&self, scope: &str, auths: &[AuthEntry]) -> Result<PathBuf, CoreError> {
        self.live.lock().expect("live").insert(scope.to_owned());
        self.created.lock().expect("created").push((
            scope.to_owned(),
            auths
                .iter()
                .map(|a| format!("{}:{}", a.registry_key, a.username))
                .collect(),
        ));
        self.secrets
            .lock()
            .expect("secrets")
            .extend(auths.iter().map(|a| a.secret.expose().to_owned()));
        Ok(PathBuf::from(format!("/fake/run/docker-{scope}")))
    }

    fn remove(&self, scope: &str) {
        self.live.lock().expect("live").remove(scope);
    }
}
