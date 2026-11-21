//! Fake [`AuthorizedKeysPort`]: an in-memory file with the same edit contract as the real one.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::CoreError;
use crate::ports::{AuthorizedKeysPort, HostUsers, KeysEditFn, KeysUpdated, SshdInfo, UnixUser};

#[derive(Debug)]
pub struct FakeAuthorizedKeys {
    users: std::sync::Arc<super::users::FakeHostUsers>,
    file: Mutex<Option<Vec<u8>>>,
    writes: Mutex<u32>,
    fail_next: Mutex<Option<CoreError>>,
    sshd: Mutex<Option<SshdInfo>>,
}

impl FakeAuthorizedKeys {
    pub fn new(users: std::sync::Arc<super::users::FakeHostUsers>) -> Self {
        Self {
            users,
            file: Mutex::new(None),
            writes: Mutex::new(0),
            fail_next: Mutex::new(None),
            sshd: Mutex::new(None),
        }
    }

    pub fn set_content(&self, bytes: Option<Vec<u8>>) {
        *self.file.lock().expect("lock") = bytes;
    }

    pub fn content(&self) -> Option<Vec<u8>> {
        self.file.lock().expect("lock").clone()
    }

    /// How many times the file was really written.
    pub fn write_count(&self) -> u32 {
        *self.writes.lock().expect("lock")
    }

    /// Make the next `update` fail before writing.
    pub fn fail_next(&self, err: CoreError) {
        *self.fail_next.lock().expect("lock") = Some(err);
    }

    pub fn set_sshd(&self, info: Option<SshdInfo>) {
        *self.sshd.lock().expect("lock") = info;
    }
}

impl AuthorizedKeysPort for FakeAuthorizedKeys {
    fn resolve(&self, user: &str) -> Result<UnixUser, CoreError> {
        self.users.lookup(user)?.ok_or_else(|| {
            CoreError::validation("user", format!("Unix user {user:?} does not exist"))
        })
    }

    fn file_path(&self, user: &UnixUser) -> PathBuf {
        user.home.join(".ssh/authorized_keys")
    }

    fn read(&self, user: &str) -> Result<Option<Vec<u8>>, CoreError> {
        self.resolve(user)?;
        Ok(self.content())
    }

    fn update(&self, user: &str, edit: &mut KeysEditFn<'_>) -> Result<KeysUpdated, CoreError> {
        self.resolve(user)?;
        if let Some(e) = self.fail_next.lock().expect("lock").take() {
            return Err(e);
        }
        let current = self.content();
        let Some(new) = edit(current.as_deref().unwrap_or_default())? else {
            return Ok(KeysUpdated {
                changed: false,
                backup_path: None,
            });
        };
        if current.as_deref() == Some(new.bytes.as_slice()) {
            return Ok(KeysUpdated {
                changed: false,
                backup_path: None,
            });
        }
        let outside = crate::authkeys::outside_block(&new.bytes)
            .map_err(|e| CoreError::conflict(e.to_string()))?;
        if outside != new.expected_outside {
            return Err(CoreError::conflict("verification failed"));
        }
        *self.writes.lock().expect("lock") += 1;
        self.set_content(Some(new.bytes));
        Ok(KeysUpdated {
            changed: true,
            backup_path: current
                .is_some()
                .then(|| PathBuf::from("authorized_keys.yanto.bak")),
        })
    }

    fn sshd_info(&self) -> Option<SshdInfo> {
        self.sshd.lock().expect("lock").clone()
    }
}
