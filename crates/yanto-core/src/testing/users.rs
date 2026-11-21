//! Fake [`HostUsers`]: a fixed account list.

use std::path::Path;
use std::sync::Mutex;

use crate::error::CoreError;
use crate::ports::{HostUsers, UnixUser};

#[derive(Debug)]
pub struct FakeHostUsers {
    users: Mutex<Vec<UnixUser>>,
}

impl FakeHostUsers {
    /// `root` and `deploy` (uid 1000) with homes under `home_root`.
    pub fn new(home_root: &Path) -> Self {
        let u = |name: &str, uid: u32, home: std::path::PathBuf, shell: &str| UnixUser {
            name: name.into(),
            uid,
            gid: uid,
            home,
            shell: shell.into(),
        };
        Self {
            users: Mutex::new(vec![
                u("root", 0, home_root.join("root"), "/bin/bash"),
                u("deploy", 1000, home_root.join("deploy"), "/bin/bash"),
            ]),
        }
    }

    pub fn add(&self, user: UnixUser) {
        self.users.lock().expect("lock").push(user);
    }
}

impl HostUsers for FakeHostUsers {
    fn lookup(&self, name: &str) -> Result<Option<UnixUser>, CoreError> {
        Ok(self
            .users
            .lock()
            .expect("lock")
            .iter()
            .find(|u| u.name == name)
            .cloned())
    }

    fn login_users(&self) -> Result<Vec<UnixUser>, CoreError> {
        Ok(self.users.lock().expect("lock").clone())
    }
}
