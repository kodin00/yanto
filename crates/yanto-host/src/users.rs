//! Unix accounts: `getpwnam` for lookups, the passwd file for the login-user dropdown.

use std::path::PathBuf;

use nix::unistd::User;
use yanto_core::CoreError;
use yanto_core::ports::{HostUsers, UnixUser};

#[derive(Debug, Clone)]
pub struct SystemUsers {
    passwd: PathBuf,
}

impl Default for SystemUsers {
    fn default() -> Self {
        Self::new("/etc/passwd")
    }
}

impl SystemUsers {
    /// `passwd` is the file listed by [`HostUsers::login_users`] (`/etc/passwd`, or a fixture).
    pub fn new(passwd: impl Into<PathBuf>) -> Self {
        Self {
            passwd: passwd.into(),
        }
    }
}

/// Parse passwd(5) text; malformed lines are skipped.
pub fn parse_passwd(text: &str) -> Vec<UnixUser> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 7 || l.starts_with('#') {
                return None;
            }
            Some(UnixUser {
                name: f[0].to_owned(),
                uid: f[2].parse().ok()?,
                gid: f[3].parse().ok()?,
                home: PathBuf::from(f[5]),
                shell: f[6].to_owned(),
            })
        })
        .collect()
}

/// `root`, or a regular account (uid 1000..65533) with a real shell.
fn can_login(u: &UnixUser) -> bool {
    let shell = u.shell.trim();
    let real_shell =
        !shell.is_empty() && !shell.ends_with("/nologin") && !shell.ends_with("/false");
    u.uid == 0 || (real_shell && (1000..65_534).contains(&u.uid))
}

impl HostUsers for SystemUsers {
    fn lookup(&self, name: &str) -> Result<Option<UnixUser>, CoreError> {
        if name.is_empty() || name.contains(['\0', ':', '/']) {
            return Ok(None);
        }
        let u = User::from_name(name).map_err(|e| {
            CoreError::new(
                yanto_core::ErrorKind::Unavailable,
                format!("cannot look up user {name:?}: {e}"),
            )
        })?;
        Ok(u.map(|u| UnixUser {
            name: u.name,
            uid: u.uid.as_raw(),
            gid: u.gid.as_raw(),
            home: u.dir,
            shell: u.shell.to_string_lossy().into_owned(),
        }))
    }

    fn login_users(&self) -> Result<Vec<UnixUser>, CoreError> {
        let text = std::fs::read_to_string(&self.passwd)
            .map_err(|e| crate::fsutil::io_err("cannot read the passwd file", e))?;
        let mut v: Vec<UnixUser> = parse_passwd(&text).into_iter().filter(can_login).collect();
        v.sort_by(|a, b| (a.uid != 0, &a.name).cmp(&(b.uid != 0, &b.name)));
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash\n\
daemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin\n\
alice:x:1000:1000:Alice,,,:/home/alice:/bin/bash\n\
svc:x:1001:1001::/home/svc:/usr/sbin/nologin\n\
nobody:x:65534:65534:nobody:/nonexistent:/bin/sh\n\
# comment\n\
broken line\n";

    #[test]
    fn login_users_are_root_and_real_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("passwd");
        std::fs::write(&p, PASSWD).unwrap();
        let names: Vec<_> = SystemUsers::new(&p)
            .login_users()
            .unwrap()
            .into_iter()
            .map(|u| u.name)
            .collect();
        assert_eq!(names, ["root", "alice"]);
    }

    #[test]
    fn lookup_resolves_root_and_rejects_nonsense() {
        let u = SystemUsers::default();
        let root = u.lookup("root").unwrap().expect("root exists");
        assert_eq!(root.uid, 0);
        assert!(u.lookup("definitely-not-a-user-xyz").unwrap().is_none());
        assert!(u.lookup("a:b").unwrap().is_none());
        assert!(u.lookup("").unwrap().is_none());
    }
}
