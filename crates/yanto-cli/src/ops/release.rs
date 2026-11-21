//! GitHub Releases access and verification for `self-update` (05 section 7).
//! Latest-version discovery prefers the `releases/latest` redirect (no API,
//! no rate limit, OQ-15); the API is used only with `GITHUB_TOKEN`, for
//! pre-releases, or for a specific tag lookup.

use std::time::Duration;

use anyhow::{Context, bail};
use semver::Version;
use sha2::{Digest, Sha256};

pub const DEFAULT_REPO: &str = "kodin00/yanto";

/// Release asset for this build's CPU architecture (D20: static musl only).
pub const fn asset_name() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "yanto-aarch64-unknown-linux-musl"
    } else {
        "yanto-x86_64-unknown-linux-musl"
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Find the digest for `name` in `sha256sum`-format text (`<hex>  <name>` or
/// `<hex> *<name>`).
pub fn digest_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

pub fn verify_checksum(sums: &str, name: &str, data: &[u8]) -> anyhow::Result<()> {
    let want =
        digest_for(sums, name).with_context(|| format!("SHA256SUMS has no entry for {name}"))?;
    let got = sha256_hex(data);
    if want != got {
        bail!("checksum mismatch for {name}: expected {want}, got {got}");
    }
    Ok(())
}

/// `https://github.com/o/r/releases/tag/v1.2.3` -> `v1.2.3`.
pub fn tag_from_location(location: &str) -> Option<String> {
    let (_, tag) = location.rsplit_once("/releases/tag/")?;
    let tag = tag.split(['?', '#']).next()?;
    (!tag.is_empty()).then(|| tag.to_string())
}

pub fn parse_tag(tag: &str) -> anyhow::Result<Version> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag))
        .with_context(|| format!("release tag {tag:?} is not a semantic version"))
}

pub fn asset_url(repo: &str, tag: &str, name: &str) -> String {
    format!("https://github.com/{repo}/releases/download/{tag}/{name}")
}

/// Which source to use to find the target tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolve {
    /// Redirect-based, no API.
    LatestRedirect,
    /// API `releases/latest`.
    LatestApi,
    /// API list, newest non-draft (includes pre-releases).
    NewestIncludingPre,
    Tag(String),
}

pub fn choose_resolver(version: Option<&str>, allow_pre: bool, have_token: bool) -> Resolve {
    match (version, allow_pre, have_token) {
        (Some(v), _, _) => Resolve::Tag(v.to_string()),
        (None, true, _) => Resolve::NewestIncludingPre,
        (None, false, true) => Resolve::LatestApi,
        (None, false, false) => Resolve::LatestRedirect,
    }
}

fn client(redirects: bool) -> anyhow::Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .user_agent(concat!("yanto-self-update/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(15))
        .https_only(true);
    if !redirects {
        b = b.redirect(reqwest::redirect::Policy::none());
    }
    Ok(b.build()?)
}

/// Resolve the release tag to install.
pub async fn resolve_tag(repo: &str, how: &Resolve, token: Option<&str>) -> anyhow::Result<String> {
    match how {
        Resolve::Tag(t) => Ok(t.clone()),
        Resolve::LatestRedirect => {
            let c = client(false)?;
            let resp = c
                .get(format!("https://github.com/{repo}/releases/latest"))
                .send()
                .await
                .context("requesting releases/latest")?;
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .context("releases/latest gave no redirect (no published release?)")?;
            tag_from_location(loc).with_context(|| format!("unexpected redirect target {loc}"))
        }
        Resolve::LatestApi => api_tag(repo, "releases/latest", token, false).await,
        Resolve::NewestIncludingPre => api_tag(repo, "releases?per_page=30", token, true).await,
    }
}

async fn api_tag(
    repo: &str,
    path: &str,
    token: Option<&str>,
    list: bool,
) -> anyhow::Result<String> {
    let c = client(true)?;
    let mut req = c
        .get(format!("https://api.github.com/repos/{repo}/{path}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.context("calling the GitHub API")?;
    let status = resp.status();
    if !status.is_success() {
        bail!(
            "GitHub API returned {status} (rate limit? set GITHUB_TOKEN, or omit --allow-prerelease to use the redirect path)"
        );
    }
    let v: serde_json::Value = resp.json().await.context("parsing GitHub API response")?;
    let tag = if list {
        v.as_array()
            .and_then(|a| a.iter().find(|r| r["draft"] != true))
            .and_then(|r| r["tag_name"].as_str())
    } else {
        v["tag_name"].as_str()
    };
    tag.map(str::to_string).context("no release found")
}

pub async fn download(url: &str) -> anyhow::Result<Vec<u8>> {
    let resp = client(true)?
        .get(url)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    Ok(resp.bytes().await?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMS: &str = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  yanto-x86_64-unknown-linux-musl
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb *yanto-aarch64-unknown-linux-musl
";

    #[test]
    fn digest_lookup_handles_text_and_binary_markers() {
        assert_eq!(
            digest_for(SUMS, "yanto-x86_64-unknown-linux-musl").unwrap(),
            "a".repeat(64)
        );
        assert_eq!(
            digest_for(SUMS, "yanto-aarch64-unknown-linux-musl").unwrap(),
            "b".repeat(64)
        );
        assert!(digest_for(SUMS, "other").is_none());
        assert!(digest_for("short  yanto", "yanto").is_none());
    }

    #[test]
    fn checksum_verification() {
        let data = b"hello";
        let sums = format!("{}  yanto-x\n", sha256_hex(data));
        verify_checksum(&sums, "yanto-x", data).unwrap();
        assert!(verify_checksum(&sums, "yanto-x", b"tampered").is_err());
        assert!(verify_checksum(&sums, "missing", data).is_err());
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn tag_parsing() {
        assert_eq!(
            tag_from_location("https://github.com/kodin00/yanto/releases/tag/v1.0.0").unwrap(),
            "v1.0.0"
        );
        assert!(tag_from_location("https://github.com/kodin00/yanto/releases").is_none());
        assert_eq!(parse_tag("v1.2.3").unwrap(), Version::new(1, 2, 3));
        assert!(parse_tag("v1.2.3-rc.1").unwrap() < Version::new(1, 2, 3));
        assert!(parse_tag("latest").is_err());
    }

    #[test]
    fn resolver_choice_prefers_redirect_without_token() {
        assert_eq!(choose_resolver(None, false, false), Resolve::LatestRedirect);
        assert_eq!(choose_resolver(None, false, true), Resolve::LatestApi);
        assert_eq!(
            choose_resolver(None, true, false),
            Resolve::NewestIncludingPre
        );
        assert_eq!(
            choose_resolver(Some("v1.0.0"), true, true),
            Resolve::Tag("v1.0.0".into())
        );
    }

    #[test]
    fn urls_are_stable() {
        assert_eq!(
            asset_url("o/r", "v1.0.0", "yanto-x"),
            "https://github.com/o/r/releases/download/v1.0.0/yanto-x"
        );
    }
}
