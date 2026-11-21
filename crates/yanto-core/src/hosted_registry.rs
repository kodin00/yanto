//! Pure pieces of the Hosted Registry (docs/rewrite/04 section 4): file rendering, bcrypt
//! hashing, and the name rules that keep user input out of URLs and htpasswd lines.

use crate::error::CoreError;

pub const INTERNAL_USER: &str = "yanto-internal";
pub const DEFAULT_PORT: u16 = 5000;
pub const DEFAULT_CONTAINER: &str = "yanto-registry";
/// Image tag fixed by D10; pinned by digest after the first pull (OQ-21).
pub const IMAGE_TAG: &str = "registry:2";
/// Where Yanto mounts its config inside the container (passed explicitly as the command).
pub const CONTAINER_CONFIG: &str = "/etc/yanto/config.yml";
/// bcrypt cost for registry users (04 4.2: 10).
pub const BCRYPT_COST: u32 = 10;

/// Media types accepted when reading manifests (04 4.3).
pub const MANIFEST_ACCEPT: &str = "application/vnd.docker.distribution.manifest.v2+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json";

/// `config.yml` (04 4.2). `readonly` is set only for the garbage-collection window.
pub fn render_config(readonly: bool) -> String {
    let mut s = String::from(
        "version: 0.1\n\
         log:\n  level: info\n\
         storage:\n  filesystem:\n    rootdirectory: /var/lib/registry\n  delete:\n    enabled: true\n",
    );
    if readonly {
        s.push_str("  maintenance:\n    readonly:\n      enabled: true\n");
    }
    s.push_str(
        "http:\n  addr: :5000\n\
         auth:\n  htpasswd:\n    realm: Yanto Registry\n    path: /auth/htpasswd\n",
    );
    s
}

/// bcrypt with the `$2y$` prefix `htpasswd -B` produces.
pub fn hash_password(password: &str, cost: u32) -> Result<String, CoreError> {
    let parts = bcrypt::hash_with_result(password, cost)
        .map_err(|e| CoreError::internal("password hashing failed", e))?;
    Ok(parts.format_for_version(bcrypt::Version::TwoY))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

/// One `name:hash` line per entry, in the given order, ending with a newline.
pub fn render_htpasswd<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut s = String::new();
    for (name, hash) in entries {
        s.push_str(name);
        s.push(':');
        s.push_str(hash);
        s.push('\n');
    }
    s
}

/// `^[a-z0-9._-]{1,32}$` (04 4.2); the internal name is reserved.
pub fn validate_username(name: &str) -> Result<String, CoreError> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        });
    if !ok {
        return Err(CoreError::validation(
            "username",
            "username must be 1-32 characters of a-z, 0-9, '.', '_' or '-'",
        ));
    }
    if name == INTERNAL_USER {
        return Err(CoreError::validation("username", "this name is reserved"));
    }
    Ok(name.to_owned())
}

/// Chosen passwords: 12-72 characters (bcrypt ignores bytes past 72), no control characters.
pub fn validate_password(pw: &str) -> Result<(), CoreError> {
    if pw.chars().count() < 12 || pw.len() > 72 || pw.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "password",
            "password must be 12-72 characters on one line",
        ));
    }
    Ok(())
}

/// A random 24-character password from an unambiguous alphabet.
pub fn generate_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let bytes = crate::crypto::random_bytes::<24>();
    bytes
        .iter()
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect()
}

/// Repository name per the distribution grammar: lowercase path components of
/// `[a-z0-9]+([._-][a-z0-9]+)*` (also `__`), at most 255 characters in total.
pub fn validate_repo(name: &str) -> Result<(), CoreError> {
    let bad = || CoreError::validation("repo", "invalid repository name");
    if name.is_empty() || name.len() > 255 {
        return Err(bad());
    }
    for comp in name.split('/') {
        let b = comp.as_bytes();
        let alnum = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
        if b.is_empty()
            || !alnum(b[0])
            || !alnum(b[b.len() - 1])
            || !b
                .iter()
                .all(|c| alnum(*c) || matches!(c, b'.' | b'_' | b'-'))
        {
            return Err(bad());
        }
    }
    Ok(())
}

/// Tag grammar: `[A-Za-z0-9_][A-Za-z0-9._-]{0,127}`.
pub fn validate_tag(tag: &str) -> Result<(), CoreError> {
    let b = tag.as_bytes();
    let ok = !b.is_empty()
        && b.len() <= 128
        && (b[0].is_ascii_alphanumeric() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(CoreError::validation("tag", "invalid tag"))
    }
}

pub fn validate_digest(d: &str) -> Result<(), CoreError> {
    match d.strip_prefix("sha256:") {
        Some(h) if h.len() == 64 && h.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) => {
            Ok(())
        }
        _ => Err(CoreError::validation("digest", "invalid digest")),
    }
}

/// `127.0.0.1:<port>` as written in image references.
pub fn local_host(port: u16) -> String {
    format!("127.0.0.1:{port}")
}

/// Whether an image reference names the Hosted Registry on `port` (`127.0.0.1` or `localhost`).
pub fn is_local_ref(image_ref: &str, port: u16) -> bool {
    let host = crate::deploy::compose::registry_host(image_ref);
    host == local_host(port) || host == format!("localhost:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_has_delete_auth_and_optional_readonly() {
        let c = render_config(false);
        assert!(c.contains("delete:\n    enabled: true"));
        assert!(c.contains("path: /auth/htpasswd"));
        assert!(!c.contains("readonly"));
        let ro = render_config(true);
        assert!(ro.contains("maintenance:\n    readonly:\n      enabled: true"));
    }

    #[test]
    fn hashes_are_2y_and_verify() {
        let h = hash_password("correct horse battery", 4).unwrap();
        assert!(h.starts_with("$2y$04$"));
        assert!(verify_password("correct horse battery", &h));
        assert!(!verify_password("nope", &h));
        assert_eq!(render_htpasswd([("a", "h1"), ("b", "h2")]), "a:h1\nb:h2\n");
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_username("ci-bot_1").is_ok());
        assert!(validate_username("Bad").is_err());
        assert!(validate_username("a:b").is_err());
        assert!(validate_username(INTERNAL_USER).is_err());
        assert!(validate_repo("team/app-1").is_ok());
        for bad in ["", "A", "a//b", "a/../b", "-a", "a/", "a b"] {
            assert!(validate_repo(bad).is_err(), "{bad}");
        }
        assert!(validate_tag("v1.2_x-3").is_ok());
        assert!(validate_tag("-x").is_err());
        assert!(validate_tag("a/b").is_err());
        assert!(validate_digest(&format!("sha256:{}", "a".repeat(64))).is_ok());
        assert!(validate_digest("sha256:XYZ").is_err());
        assert_eq!(generate_password().len(), 24);
    }

    #[test]
    fn local_refs() {
        assert!(is_local_ref("127.0.0.1:5055/app:1", 5055));
        assert!(!is_local_ref("127.0.0.1:5055/app:1", 5000));
        assert!(!is_local_ref("nginx", 5000));
    }
}
