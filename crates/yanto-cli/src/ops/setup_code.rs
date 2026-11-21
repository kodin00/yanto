//! One-time setup code (05 section 3.6, D18).
//!
//! TODO(M1): persisting the hash and the "owner exists" check belong to the
//! auth module. This file only owns generation, hashing and the two stubs the
//! installer calls, so M1 can swap the stubs without touching the wizard.

// TODO(M1): `generate` and `hash` are consumed once `issue` persists codes.
#![allow(dead_code)]

use std::path::Path;

use sha2::{Digest, Sha256};

/// Unambiguous alphabet (no 0/O/1/I), 32 symbols so a byte maps without bias.
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// 16 symbols grouped `XXXX-XXXX-XXXX-XXXX` (80 bits).
pub fn generate() -> anyhow::Result<String> {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|e| anyhow::anyhow!("no secure randomness: {e}"))?;
    let chars: Vec<char> = raw
        .iter()
        .map(|b| ALPHABET[(*b & 31) as usize] as char)
        .collect();
    Ok(chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-"))
}

/// SHA-256 hex of the normalised code (dashes and case ignored).
pub fn hash(code: &str) -> String {
    let norm: String = code
        .chars()
        .filter(|c| *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    Sha256::digest(norm.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// TODO(M1): true when an Owner already exists in the DB.
pub fn owner_exists(_db_path: &Path) -> anyhow::Result<bool> {
    Ok(false)
}

/// TODO(M1): generate a code, store only [`hash`] in the DB (valid until used
/// or 24 h), and return the plaintext to print once. Until the auth tables
/// exist this returns `None` and the installer tells the operator to run
/// `yanto setup-code`.
pub fn issue(_db_path: &Path) -> anyhow::Result<Option<String>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_shape_and_alphabet() {
        let c = generate().unwrap();
        assert_eq!(c.len(), 19);
        let groups: Vec<&str> = c.split('-').collect();
        assert_eq!(groups.len(), 4);
        assert!(
            c.chars()
                .all(|ch| ch == '-' || ALPHABET.contains(&(ch as u8)))
        );
        assert_ne!(generate().unwrap(), c);
    }

    #[test]
    fn hash_ignores_dashes_and_case() {
        assert_eq!(hash("ABCD-EFGH"), hash("abcdefgh"));
        assert_eq!(hash("x").len(), 64);
    }
}
