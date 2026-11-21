//! Deploy Key generation and import (ed25519 only, via the `ssh-key` crate; no `ssh-keygen`).

use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey, rand_core::OsRng};

use crate::error::CoreError;
use crate::secrets::Secret;

pub struct GeneratedKey {
    pub private_openssh: Secret,
    pub public_key: String,
    pub fingerprint: String,
}

fn describe(mut key: PrivateKey, comment: &str) -> Result<GeneratedKey, CoreError> {
    key.set_comment(comment);
    let private = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| CoreError::internal("cannot encode private key", e))?;
    let public = key
        .public_key()
        .to_openssh()
        .map_err(|e| CoreError::internal("cannot encode public key", e))?;
    Ok(GeneratedKey {
        private_openssh: Secret::new(private.as_str()),
        public_key: public,
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
    })
}

pub fn generate_ed25519(comment: &str) -> Result<GeneratedKey, CoreError> {
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .map_err(|e| CoreError::internal("cannot generate key", e))?;
    describe(key, comment)
}

/// Parse an unencrypted OpenSSH ed25519 private key.
pub fn import_openssh(text: &Secret, comment: &str) -> Result<GeneratedKey, CoreError> {
    let key = PrivateKey::from_openssh(text.expose().trim())
        .map_err(|_| CoreError::validation("private_key", "not a valid OpenSSH private key"))?;
    if key.is_encrypted() {
        return Err(CoreError::validation(
            "private_key",
            "passphrase-protected keys are not supported",
        ));
    }
    if key.algorithm() != Algorithm::Ed25519 {
        return Err(CoreError::validation(
            "private_key",
            "only ed25519 keys are supported",
        ));
    }
    describe(key, comment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_round_trips_through_import() {
        let g = generate_ed25519("yanto:test").unwrap();
        assert!(g.public_key.starts_with("ssh-ed25519 "));
        assert!(g.fingerprint.starts_with("SHA256:"));
        assert!(
            g.private_openssh
                .expose()
                .contains("BEGIN OPENSSH PRIVATE KEY")
        );
        let imported = import_openssh(&g.private_openssh, "yanto:test").unwrap();
        assert_eq!(imported.fingerprint, g.fingerprint);
        assert!(!format!("{:?}", g.private_openssh).contains("OPENSSH"));
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(import_openssh(&Secret::new("nope"), "c").is_err());
    }
}
