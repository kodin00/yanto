//! Input rules for Projects (docs/rewrite/04 1.1 "Source rules"). Pure functions, no I/O.

use crate::error::CoreError;

/// What free-form source input turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classified {
    Git(String),
    Image(String),
}

/// Legacy-compatible classification of one text box: `docker run ...` is rejected with a hint,
/// anything that parses as an image reference is an image Project, the rest is a Git URL.
pub fn classify_source(input: &str) -> Result<Classified, CoreError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(CoreError::validation(
            "source",
            "enter a Git URL or an image reference",
        ));
    }
    if s.starts_with("docker ") {
        return Err(CoreError::validation(
            "source",
            "enter an image reference such as nginx:alpine, not a docker command",
        ));
    }
    if s.contains("://") || is_scp_like(s) || s.ends_with(".git") {
        return Ok(Classified::Git(s.to_owned()));
    }
    if image_ref(s).is_ok() {
        return Ok(Classified::Image(s.to_owned()));
    }
    Ok(Classified::Git(s.to_owned()))
}

fn is_scp_like(s: &str) -> bool {
    let Some((user_host, path)) = s.split_once(':') else {
        return false;
    };
    let Some((user, host)) = user_host.split_once('@') else {
        return false;
    };
    !user.is_empty() && !host.is_empty() && !host.contains('/') && !path.starts_with("//")
}

/// Allowed Git URLs: `ssh://`, scp-style `user@host:path`, `https://` without credentials.
pub fn git_url(raw: &str) -> Result<String, CoreError> {
    let s = raw.trim();
    let bad = |m: &str| Err(CoreError::validation("git_url", m.to_owned()));
    if s.is_empty() || s.len() > 2048 {
        return bad("Git URL must be 1-2048 characters");
    }
    if s.starts_with('-') {
        return bad("Git URL must not start with '-'");
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return bad("Git URL must not contain whitespace");
    }
    if let Some(rest) = s.strip_prefix("https://") {
        let authority = rest.split('/').next().unwrap_or("");
        if authority.is_empty() || authority.contains('@') {
            return bad("https Git URLs must name a host and carry no credentials");
        }
        return Ok(s.to_owned());
    }
    if let Some(rest) = s.strip_prefix("ssh://") {
        if rest.split('/').next().is_none_or(str::is_empty) {
            return bad("ssh Git URL must name a host");
        }
        return Ok(s.to_owned());
    }
    if s.contains("://") || s.starts_with("ext::") || s.starts_with("file:") {
        return bad("only ssh://, https:// and user@host:path Git URLs are allowed");
    }
    if is_scp_like(s) {
        return Ok(s.to_owned());
    }
    bad("only ssh://, https:// and user@host:path Git URLs are allowed")
}

/// Branch, tag or commit: `^[A-Za-z0-9._/@-]+$`, no leading `-`, no `..`.
pub fn git_ref(field: &str, raw: &str) -> Result<String, CoreError> {
    let s = raw.trim();
    if s.is_empty()
        || s.len() > 255
        || s.starts_with('-')
        || s.contains("..")
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '@' | '-'))
    {
        return Err(CoreError::validation(
            field,
            "must be 1-255 characters of letters, digits and . _ / @ - (no leading '-', no '..')",
        ));
    }
    Ok(s.to_owned())
}

/// `[registry[:port]/]path[:tag][@sha256:hex]`.
pub fn image_ref(raw: &str) -> Result<String, CoreError> {
    let s = raw.trim();
    let bad = || {
        CoreError::validation(
            "image_ref",
            "must look like nginx:alpine, ghcr.io/org/app:1.2 or repo@sha256:<digest>",
        )
    };
    if s.is_empty() || s.len() > 255 || s.starts_with('-') || s.starts_with('/') || s.contains("//")
    {
        return Err(bad());
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | ':' | '@' | '-'))
    {
        return Err(bad());
    }
    let (name_tag, digest) = match s.split_once('@') {
        Some((n, d)) => (n, Some(d)),
        None => (s, None),
    };
    if let Some(d) = digest {
        let ok = d
            .strip_prefix("sha256:")
            .is_some_and(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()));
        if !ok {
            return Err(bad());
        }
    }
    let last = name_tag.rsplit('/').next().unwrap_or("");
    if last.is_empty() || last.starts_with(':') || name_tag.ends_with(':') {
        return Err(bad());
    }
    if name_tag.chars().any(|c| c.is_ascii_uppercase())
        && name_tag
            .split('/')
            .skip(usize::from(name_tag.contains('/')))
            .any(|part| part.chars().any(|c| c.is_ascii_uppercase()))
    {
        return Err(bad());
    }
    Ok(s.to_owned())
}

/// Relative compose path ending in `.yml`/`.yaml`.
pub fn compose_path(raw: &str) -> Result<String, CoreError> {
    let s = raw.trim();
    let ok = relative_path_ok(s)
        && (s.ends_with(".yml") || s.ends_with(".yaml"))
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
    if ok {
        Ok(s.to_owned())
    } else {
        Err(CoreError::validation(
            "compose_path",
            "must be a relative path ending in .yml or .yaml (letters, digits and . _ / -)",
        ))
    }
}

/// Relative env file whose basename starts with `.env`.
pub fn env_file(raw: &str) -> Result<String, CoreError> {
    let s = raw.trim();
    let base = s.rsplit('/').next().unwrap_or("");
    let ok = relative_path_ok(s)
        && base.starts_with(".env")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
    if ok {
        Ok(s.to_owned())
    } else {
        Err(CoreError::validation(
            "env_file",
            "must be a relative path whose file name starts with .env",
        ))
    }
}

fn relative_path_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && !s.starts_with('/')
        && !s.starts_with('-')
        && !s.contains("..")
        && !s.contains("//")
        && !s.ends_with('/')
}

pub fn name(raw: &str) -> Result<String, CoreError> {
    let s = raw.trim();
    if s.is_empty() || s.chars().count() > 100 || s.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "name",
            "name must be 1-100 characters",
        ));
    }
    Ok(s.to_owned())
}

/// `[a-z0-9][a-z0-9_-]*`, at most 63 characters.
pub fn slug(raw: &str) -> Result<String, CoreError> {
    let s = raw.trim();
    let ok = !s.is_empty()
        && s.len() <= 63
        && s.chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'));
    if ok {
        Ok(s.to_owned())
    } else {
        Err(CoreError::validation(
            "slug",
            "slug must be 1-63 characters of a-z, 0-9, _ and -, starting with a letter or digit",
        ))
    }
}

/// A slug derived from a display name (`My App!` becomes `my-app`).
pub fn slug_from_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    out.chars()
        .take(63)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_like_legacy() {
        assert_eq!(
            classify_source("nginx:alpine").unwrap(),
            Classified::Image("nginx:alpine".into())
        );
        assert_eq!(
            classify_source("ghcr.io/org/app:1.2").unwrap(),
            Classified::Image("ghcr.io/org/app:1.2".into())
        );
        assert_eq!(
            classify_source("git@github.com:org/repo.git").unwrap(),
            Classified::Git("git@github.com:org/repo.git".into())
        );
        assert_eq!(
            classify_source("https://github.com/org/repo").unwrap(),
            Classified::Git("https://github.com/org/repo".into())
        );
        assert!(classify_source("docker run nginx").is_err());
        assert!(classify_source("  ").is_err());
    }

    #[test]
    fn git_urls_are_allow_listed() {
        for ok in [
            "git@github.com:org/repo.git",
            "ssh://git@host:2222/org/repo.git",
            "https://github.com/org/repo.git",
        ] {
            assert!(git_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "file:///etc",
            "ext::sh -c id",
            "-oProxyCommand=x",
            "http://github.com/x",
            "https://user:token@github.com/x",
            "git://host/x",
            "/local/path",
            "a b",
        ] {
            assert!(git_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn refs_and_paths() {
        assert!(git_ref("branch", "feature/x-1.2").is_ok());
        for bad in ["-x", "a..b", "a b", "", "a;b"] {
            assert!(git_ref("branch", bad).is_err(), "{bad}");
        }
        assert!(compose_path("deploy/docker-compose.yml").is_ok());
        for bad in ["/abs.yml", "../x.yml", "a.txt", "a b.yml", "a//b.yml"] {
            assert!(compose_path(bad).is_err(), "{bad}");
        }
        assert!(env_file(".env").is_ok());
        assert!(env_file("cfg/.env.prod").is_ok());
        assert!(env_file("secrets.txt").is_err());
        assert!(env_file("../.env").is_err());
    }

    #[test]
    fn image_refs() {
        for ok in [
            "nginx",
            "nginx:alpine",
            "127.0.0.1:5000/app:v1",
            "ghcr.io/org/app@sha256:0000000000000000000000000000000000000000000000000000000000000000",
        ] {
            assert!(image_ref(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "-x",
            "a b",
            "app@sha256:short",
            "Org/App:tag",
            "a//b",
            "app:",
        ] {
            assert!(image_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("my-app_1").unwrap(), "my-app_1");
        for bad in ["", "-a", "A", "a b", &"a".repeat(64)] {
            assert!(slug(bad).is_err(), "{bad}");
        }
        assert_eq!(slug_from_name("My App!"), "my-app");
        assert_eq!(slug_from_name("  Hello   World  "), "hello-world");
    }
}
