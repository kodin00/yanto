//! All M5 fakes wired together, so a test can inspect what the services asked the Host to do.

use std::path::Path;
use std::sync::Arc;

use super::authkeys::FakeAuthorizedKeys;
use super::backup::{FakeBackupHost, FakeProjectBackups};
use super::cron::{FakeCron, FakeCronExec};
use super::users::FakeHostUsers;
use crate::ports::HostMgmtPorts;

pub struct HostFakes {
    pub users: Arc<FakeHostUsers>,
    pub cron: Arc<FakeCron>,
    pub cron_exec: Arc<FakeCronExec>,
    pub authorized_keys: Arc<FakeAuthorizedKeys>,
    pub backup_host: Arc<FakeBackupHost>,
    pub project_backups: Arc<FakeProjectBackups>,
}

impl HostFakes {
    pub fn new(home_root: &Path) -> Self {
        let users = Arc::new(FakeHostUsers::new(home_root));
        Self {
            authorized_keys: Arc::new(FakeAuthorizedKeys::new(users.clone())),
            users,
            cron: Arc::default(),
            cron_exec: Arc::default(),
            backup_host: Arc::default(),
            project_backups: Arc::default(),
        }
    }

    pub fn ports(&self) -> HostMgmtPorts {
        HostMgmtPorts {
            users: self.users.clone(),
            cron: self.cron.clone(),
            cron_exec: self.cron_exec.clone(),
            authorized_keys: self.authorized_keys.clone(),
            backup_host: self.backup_host.clone(),
            project_backups: self.project_backups.clone(),
        }
    }
}
