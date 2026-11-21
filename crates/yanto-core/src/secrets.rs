//! Secrets at rest: AES-256-GCM envelope (docs/rewrite/01 section 8.3, 02 section 3).
//!
//! Envelope: `enc:v1:<base64url nonce>:<base64url ciphertext||tag>`. The AAD is
//! `<table>.<column>.<row id>`, so a ciphertext copied to another row or column
//! fails to open. The key is 32 random bytes in `<data_dir>/keys/app.key`
//! (`0600`, directory `0700`), generated if absent, never read from env or flags.

use std::fmt;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, Generate, Key, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use zeroize::Zeroizing;

use crate::error::CoreError;

const KEY_LEN: usize = 32;
const PREFIX: &str = "enc:v1:";

/// A string that never prints its value (`Debug` and `Display` are redacted).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only way to read the value; call sites are greppable.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

/// Envelope encryption with the instance key.
pub struct Secrets {
    cipher: Aes256Gcm,
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secrets([REDACTED])")
    }
}

/// Location of the key file inside the data dir.
pub fn key_path(data_dir: &Path) -> PathBuf {
    data_dir.join("keys").join("app.key")
}

impl Secrets {
    /// Build from raw key bytes (tests, or after reading the key file).
    pub fn from_key_bytes(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() != KEY_LEN {
            return Err(CoreError::new(
                crate::ErrorKind::Internal,
                format!("app.key must be exactly {KEY_LEN} bytes"),
            ));
        }
        let key = Key::<Aes256Gcm>::try_from(bytes)
            .map_err(|_| CoreError::new(crate::ErrorKind::Internal, "invalid app.key"))?;
        Ok(Self {
            cipher: Aes256Gcm::new(&key),
        })
    }

    /// An in-memory random key for tests.
    pub fn ephemeral() -> Self {
        let key = crate::crypto::random_bytes::<KEY_LEN>();
        Self::from_key_bytes(&key).expect("32 bytes")
    }

    /// Load `<data_dir>/keys/app.key`, generating it (0600) if missing. An
    /// existing key readable by group/other is refused.
    pub fn load_or_create(data_dir: &Path) -> Result<Self, CoreError> {
        let path = key_path(data_dir);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                check_key_permissions(&path)?;
                Self::from_key_bytes(&bytes)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = Zeroizing::new(crate::crypto::random_bytes::<KEY_LEN>());
                write_new_key(&path, &key[..])?;
                tracing::info!(path = %path.display(), "generated new app.key");
                Self::from_key_bytes(&key[..])
            }
            Err(e) => Err(CoreError::internal("cannot read app.key", e)),
        }
    }

    /// Encrypt `plaintext`, binding it to `table.column.row_id`.
    pub fn seal(
        &self,
        table: &str,
        column: &str,
        row_id: &str,
        plaintext: &Secret,
    ) -> Result<String, CoreError> {
        let nonce = Nonce::generate();
        let aad = aad(table, column, row_id);
        let ct = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.expose().as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| CoreError::new(crate::ErrorKind::Internal, "encryption failed"))?;
        Ok(format!(
            "{PREFIX}{}:{}",
            URL_SAFE_NO_PAD.encode(nonce),
            URL_SAFE_NO_PAD.encode(ct)
        ))
    }

    /// Decrypt an envelope produced by [`Secrets::seal`] for the same location.
    pub fn open(
        &self,
        table: &str,
        column: &str,
        row_id: &str,
        envelope: &str,
    ) -> Result<Secret, CoreError> {
        let bad = || CoreError::new(crate::ErrorKind::Internal, "secret cannot be decrypted");
        let rest = envelope.strip_prefix(PREFIX).ok_or_else(bad)?;
        let (n, c) = rest.split_once(':').ok_or_else(bad)?;
        let nonce_bytes = URL_SAFE_NO_PAD.decode(n).map_err(|_| bad())?;
        let ct = URL_SAFE_NO_PAD.decode(c).map_err(|_| bad())?;
        let nonce = Nonce::try_from(nonce_bytes.as_slice()).map_err(|_| bad())?;
        let aad = aad(table, column, row_id);
        let pt = self
            .cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &ct,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| bad())?;
        String::from_utf8(pt).map(Secret::new).map_err(|_| bad())
    }
}

fn aad(table: &str, column: &str, row_id: &str) -> String {
    format!("{table}.{column}.{row_id}")
}

#[cfg(unix)]
fn check_key_permissions(path: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|e| CoreError::internal("cannot stat app.key", e))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(CoreError::new(
            crate::ErrorKind::Internal,
            format!(
                "refusing to use {}: mode {:o} is accessible to group/other (expected 0600)",
                path.display(),
                mode & 0o777
            ),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_key_permissions(_path: &Path) -> Result<(), CoreError> {
    Ok(())
}

fn write_new_key(path: &Path, key: &[u8]) -> Result<(), CoreError> {
    use std::io::Write;
    let dir = path.parent().expect("key path has a parent");
    std::fs::create_dir_all(dir).map_err(|e| CoreError::internal("cannot create keys dir", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| CoreError::internal("cannot restrict keys dir", e))?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| CoreError::internal("cannot create app.key", e))?;
        f.write_all(key)
            .and_then(|()| f.sync_all())
            .map_err(|e| CoreError::internal("cannot write app.key", e))
    }
    #[cfg(not(unix))]
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| CoreError::internal("cannot create app.key", e))?;
        f.write_all(key)
            .map_err(|e| CoreError::internal("cannot write app.key", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_aad_binding() {
        let s = Secrets::ephemeral();
        let env = s
            .seal(
                "projects",
                "deploy_token_enc",
                "prj_1",
                &Secret::new("hunter2"),
            )
            .unwrap();
        assert!(env.starts_with("enc:v1:"));
        assert!(!env.contains("hunter2"));
        assert_eq!(
            s.open("projects", "deploy_token_enc", "prj_1", &env)
                .unwrap()
                .expose(),
            "hunter2"
        );
        assert!(
            s.open("projects", "deploy_token_enc", "prj_2", &env)
                .is_err()
        );
        assert!(s.open("projects", "other_enc", "prj_1", &env).is_err());
    }

    #[test]
    fn nonces_differ_and_tampering_fails() {
        let s = Secrets::ephemeral();
        let v = Secret::new("x");
        let a = s.seal("t", "c", "1", &v).unwrap();
        let b = s.seal("t", "c", "1", &v).unwrap();
        assert_ne!(a, b);
        let mut broken = a.clone();
        broken.push('A');
        assert!(s.open("t", "c", "1", &broken).is_err());
        assert!(s.open("t", "c", "1", "garbage").is_err());
    }

    #[test]
    fn debug_and_display_are_redacted() {
        let v = Secret::new("topsecret");
        assert!(!format!("{v:?}").contains("topsecret"));
        assert!(!format!("{v}").contains("topsecret"));
    }

    #[test]
    fn key_is_generated_once_with_0600_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let a = Secrets::load_or_create(dir.path()).unwrap();
        let path = key_path(dir.path());
        assert_eq!(std::fs::read(&path).unwrap().len(), 32);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let dmode = std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dmode, 0o700);
        }
        let env = a.seal("t", "c", "1", &Secret::new("v")).unwrap();
        let b = Secrets::load_or_create(dir.path()).unwrap();
        assert_eq!(b.open("t", "c", "1", &env).unwrap().expose(), "v");
    }

    #[cfg(unix)]
    #[test]
    fn world_readable_key_is_refused_and_wrong_length_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        Secrets::load_or_create(dir.path()).unwrap();
        let path = key_path(dir.path());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Secrets::load_or_create(dir.path()).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&path, b"short").unwrap();
        assert!(Secrets::load_or_create(dir.path()).is_err());
    }
}
