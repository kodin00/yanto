//! Password hashing: Argon2id in PHC string format, run off the async runtime.

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Argon2, Params};

use crate::error::CoreError;

pub const MIN_PASSWORD_CHARS: usize = 12;
pub const MAX_PASSWORD_BYTES: usize = 1024;

/// Argon2id hasher. Verification uses the parameters embedded in each stored
/// hash, so changing the cost later does not invalidate existing passwords.
#[derive(Debug, Clone)]
pub struct Passwords {
    params: Params,
    /// Verified against when the user does not exist, so unknown-user logins
    /// cost the same as wrong-password logins (no timing oracle).
    dummy_hash: String,
}

impl Default for Passwords {
    fn default() -> Self {
        Self::with_params(Params::default())
    }
}

impl Passwords {
    pub fn with_params(params: Params) -> Self {
        let dummy_hash = hash_sync(&params, "yanto-dummy-password-never-valid")
            .expect("argon2 parameters are valid");
        Self { params, dummy_hash }
    }

    /// Minimal-cost parameters for tests only.
    pub fn cheap_for_tests() -> Self {
        Self::with_params(Params::new(8, 1, 1, None).expect("valid argon2 params"))
    }

    /// Enforce the password policy (docs/rewrite/03 a.3): 12+ chars, at most 1024 bytes.
    pub fn validate_policy(password: &str) -> Result<(), CoreError> {
        if password.chars().count() < MIN_PASSWORD_CHARS {
            return Err(CoreError::validation(
                "password",
                format!("password must be at least {MIN_PASSWORD_CHARS} characters"),
            ));
        }
        if password.len() > MAX_PASSWORD_BYTES {
            return Err(CoreError::validation(
                "password",
                format!("password must be at most {MAX_PASSWORD_BYTES} bytes"),
            ));
        }
        Ok(())
    }

    pub async fn hash(&self, password: &str) -> Result<String, CoreError> {
        let params = self.params.clone();
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || hash_sync(&params, &password))
            .await
            .map_err(|e| CoreError::internal("password hashing task failed", e))?
    }

    /// `true` when `password` matches the PHC string `hash`. A malformed hash is a mismatch.
    pub async fn verify(&self, password: &str, hash: &str) -> bool {
        let password = password.to_owned();
        let hash = hash.to_owned();
        tokio::task::spawn_blocking(move || {
            Argon2::default()
                .verify_password(password.as_bytes(), hash.as_str())
                .is_ok()
        })
        .await
        .unwrap_or(false)
    }

    /// Burn the same time as a real verification and report failure.
    pub async fn verify_dummy(&self, password: &str) {
        let _ = self.verify(password, &self.dummy_hash).await;
    }
}

fn hash_sync(params: &Params, password: &str) -> Result<String, CoreError> {
    Argon2::from(params)
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| {
            CoreError::internal("cannot hash password", std::io::Error::other(e.to_string()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_is_argon2id_phc_and_verifies() {
        let p = Passwords::cheap_for_tests();
        let h = p.hash("correct horse battery").await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(p.verify("correct horse battery", &h).await);
        assert!(!p.verify("wrong horse battery", &h).await);
        assert!(!p.verify("x", "not a phc string").await);
    }

    #[tokio::test]
    async fn salts_differ() {
        let p = Passwords::cheap_for_tests();
        assert_ne!(
            p.hash("same password 123").await.unwrap(),
            p.hash("same password 123").await.unwrap()
        );
    }

    #[test]
    fn policy_bounds() {
        assert!(Passwords::validate_policy("short").is_err());
        assert!(Passwords::validate_policy("exactly12chr").is_ok());
        assert!(Passwords::validate_policy(&"a".repeat(1025)).is_err());
        assert!(Passwords::validate_policy(&"a".repeat(1024)).is_ok());
    }
}
