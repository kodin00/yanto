//! Pure logic for Ingress Providers, Host Daemons and `frps` (docs/rewrite/04 sections 8-9):
//! hostname validation, systemd output parsing, Traefik file rendering, `frps.toml` editing.
//! No I/O here; the services drive the ports in [`crate::ports`].

pub mod cloudflare;
pub mod daemon;
pub mod frps;
pub mod traefik;

use crate::error::CoreError;

/// Ingress Provider of a Hostname (ADR 0008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    CloudflareTunnel,
    Traefik,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::CloudflareTunnel => "cloudflare_tunnel",
            Provider::Traefik => "traefik",
        }
    }

    pub fn parse(s: &str) -> Result<Self, CoreError> {
        match s {
            "cloudflare_tunnel" => Ok(Provider::CloudflareTunnel),
            "traefik" => Ok(Provider::Traefik),
            _ => Err(CoreError::validation(
                "ingress_provider",
                "ingress_provider must be cloudflare_tunnel or traefik",
            )),
        }
    }
}

/// Lowercase, trim, and validate a public hostname: LDH labels, at least two labels, an alphabetic
/// TLD, 253 characters at most, no wildcard. Everything that could break out of a Traefik
/// `Host(...)` rule (backticks, quotes, spaces) is rejected here, before any file or API write.
pub fn normalize_hostname(input: &str) -> Result<String, CoreError> {
    let h = input.trim().to_ascii_lowercase();
    let bad = |m: &str| Err(CoreError::validation("hostname", m.to_owned()));
    if h.is_empty() || h.len() > 253 {
        return bad("hostname must be 1-253 characters");
    }
    if h.contains('*') {
        return bad("wildcard hostnames are not supported");
    }
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() < 2 {
        return bad("hostname needs at least two labels, like app.example.com");
    }
    for l in &labels {
        let b = l.as_bytes();
        let ok = !b.is_empty()
            && b.len() <= 63
            && b.iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
            && b[0] != b'-'
            && b[b.len() - 1] != b'-';
        if !ok {
            return bad("hostname may only contain letters, digits, hyphens and dots");
        }
    }
    let tld = labels[labels.len() - 1].as_bytes();
    if tld.len() < 2 || !tld.iter().all(u8::is_ascii_lowercase) {
        return bad("hostname must end in an alphabetic top-level domain");
    }
    Ok(h)
}

/// `yanto-` + hostname with dots turned into hyphens (Traefik router and service names).
pub fn router_name(hostname: &str) -> String {
    format!("yanto-{}", hostname.replace('.', "-"))
}

/// Does `hostname` fall inside zone `zone` (equal or a subdomain)?
pub fn in_zone(hostname: &str, zone: &str) -> bool {
    let (h, z) = (hostname.to_ascii_lowercase(), zone.to_ascii_lowercase());
    h == z || h.ends_with(&format!(".{z}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostnames_are_normalized_and_validated() {
        assert_eq!(
            normalize_hostname("  App.Example.COM ").unwrap(),
            "app.example.com"
        );
        for bad in [
            "",
            "localhost",
            "a b.example.com",
            "a`.example.com",
            "x.example.com`) || Host(`y",
            "*.example.com",
            "-a.example.com",
            "a-.example.com",
            "a..example.com",
            "app.example.c0m",
            "app.e",
            "ünï.example.com",
            "a.example.com/",
        ] {
            assert!(normalize_hostname(bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(normalize_hostname(&format!("{}.com", "a".repeat(64))).is_err());
        assert!(normalize_hostname(&format!("{}.example.com", "a".repeat(63))).is_ok());
    }

    #[test]
    fn names_and_zones() {
        assert_eq!(router_name("app.example.com"), "yanto-app-example-com");
        assert!(in_zone("app.example.com", "example.com"));
        assert!(in_zone("example.com", "Example.com"));
        assert!(!in_zone("badexample.com", "example.com"));
        assert_eq!(Provider::parse("traefik").unwrap().as_str(), "traefik");
        assert!(Provider::parse("nginx").is_err());
    }
}
