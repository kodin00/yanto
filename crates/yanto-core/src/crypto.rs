//! Small, dependency-light helpers: random tokens, ids, hashing, constant-time compare.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Fill `buf` from the OS CSPRNG. Failure means the platform has no entropy
/// source, which is unrecoverable for a credential-issuing process.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("operating system random source unavailable");
    buf
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// `n` random bytes as lowercase hex.
pub fn random_hex<const N: usize>() -> String {
    hex(&random_bytes::<N>())
}

/// Row id: `<prefix>_<24 hex>` (12 random bytes), as in docs/rewrite/02 section 1.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", random_hex::<12>())
}

/// 32 random bytes, base64url without padding (session cookie values, CSRF tokens).
pub fn random_token_b64() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

/// SHA-256 of `input` as lowercase hex.
pub fn sha256_hex(input: &str) -> String {
    hex(&Sha256::digest(input.as_bytes()))
}

/// Constant-time string equality.
pub fn ct_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_prefix_and_24_hex() {
        let id = new_id("usr");
        assert!(id.starts_with("usr_"));
        assert_eq!(id.len(), 4 + 24);
        assert_ne!(id, new_id("usr"));
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn ct_eq_works() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
    }

    #[test]
    fn tokens_are_url_safe_and_distinct() {
        let t = random_token_b64();
        assert_eq!(t.len(), 43);
        assert!(
            t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(t, random_token_b64());
    }
}
