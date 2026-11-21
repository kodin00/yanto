//! Client address resolution and the same-origin check shared by the dashboard.

use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use axum::http::{HeaderMap, header};
use yanto_core::actor::{Channel, Origin};
use yanto_core::crypto::random_hex;

/// An IP network such as `127.0.0.1/32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl FromStr for Cidr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let (a, p) = s.split_once('/').unwrap_or((s, ""));
        let addr: IpAddr = a
            .trim()
            .parse()
            .map_err(|_| format!("invalid address in {s:?}"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = if p.is_empty() {
            max
        } else {
            p.parse::<u8>()
                .map_err(|_| format!("invalid prefix in {s:?}"))?
        };
        if prefix > max {
            return Err(format!("prefix too long in {s:?}"));
        }
        Ok(Self { addr, prefix })
    }
}

impl Cidr {
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(n), IpAddr::V4(i)) => mask_eq(
                u128::from(u32::from(n)),
                u128::from(u32::from(i)),
                32,
                self.prefix,
            ),
            (IpAddr::V6(n), IpAddr::V6(i)) => {
                mask_eq(u128::from(n), u128::from(i), 128, self.prefix)
            }
            _ => false,
        }
    }
}

fn mask_eq(a: u128, b: u128, bits: u32, prefix: u8) -> bool {
    let prefix = u32::from(prefix);
    if prefix == 0 {
        return true;
    }
    let shift = bits - prefix;
    (a >> shift) == (b >> shift)
}

/// Parse a list of CIDR strings, skipping (and logging) invalid entries.
pub fn parse_cidrs<'a>(items: impl IntoIterator<Item = &'a str>) -> Vec<Cidr> {
    items
        .into_iter()
        .filter_map(|s| match s.parse() {
            Ok(c) => Some(c),
            Err(err) => {
                tracing::warn!(%err, "ignoring invalid trusted proxy entry");
                None
            }
        })
        .collect()
}

pub fn default_trusted_proxies() -> Vec<Cidr> {
    parse_cidrs(["127.0.0.1/32", "::1/128"])
}

/// Build the [`Origin`] for a request. `X-Forwarded-For` / `CF-Connecting-IP` are honored
/// only when the socket peer is a trusted proxy (docs/rewrite/03 a.4); otherwise the peer
/// address is used, so the headers cannot be spoofed to dodge rate limits.
pub fn resolve_origin(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    trusted: &[Cidr],
    channel: Channel,
) -> Origin {
    let peer_ip = peer.map(|p| p.ip());
    let ip = match peer_ip {
        Some(p) if trusted.iter().any(|c| c.contains(p)) => {
            forwarded_ip(headers, trusted).or(Some(p))
        }
        other => other,
    };
    let mut origin = Origin::new(channel, ip.map(|i| i.to_string()));
    origin.request_id = Some(format!("req_{}", random_hex::<6>()));
    origin
}

fn forwarded_ip(headers: &HeaderMap, trusted: &[Cidr]) -> Option<IpAddr> {
    if let Some(v) = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        && let Ok(ip) = v.trim().parse()
    {
        return Some(ip);
    }
    let xff = headers.get("x-forwarded-for")?.to_str().ok()?;
    // Right to left: the first address that is not itself a trusted proxy is the client.
    xff.split(',')
        .rev()
        .filter_map(|s| s.trim().parse::<IpAddr>().ok())
        .find(|ip| !trusted.iter().any(|c| c.contains(*ip)))
}

/// Same-origin check for state-changing dashboard requests (docs/rewrite/03 a.3).
///
/// Requires an `Origin` (or `Referer`) whose host matches the request `Host` or the host of
/// `public_url`, and rejects `Sec-Fetch-Site: cross-site`.
pub fn same_origin(headers: &HeaderMap, public_url: &str) -> bool {
    if headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("cross-site"))
    {
        return false;
    }
    let Some(source) = headers
        .get(header::ORIGIN)
        .or_else(|| headers.get(header::REFERER))
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(src_host) = host_of(source) else {
        return false;
    };
    let request_host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    request_host.is_some_and(|h| h.eq_ignore_ascii_case(src_host))
        || host_of(public_url).is_some_and(|h| h.eq_ignore_ascii_case(src_host))
}

/// `host[:port]` of a URL string (`scheme://host[:port]/path`).
fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn cidr_matching() {
        let c: Cidr = "10.0.0.0/8".parse().unwrap();
        assert!(c.contains("10.1.2.3".parse().unwrap()));
        assert!(!c.contains("11.0.0.1".parse().unwrap()));
        assert!(!c.contains("::1".parse().unwrap()));
        let all: Cidr = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains("8.8.8.8".parse().unwrap()));
        assert!("1.2.3.4/33".parse::<Cidr>().is_err());
        assert!(
            "::1/128"
                .parse::<Cidr>()
                .unwrap()
                .contains("::1".parse().unwrap())
        );
    }

    #[test]
    fn forwarded_headers_only_trusted_from_trusted_peers() {
        let trusted = default_trusted_proxies();
        let h = headers(&[("x-forwarded-for", "203.0.113.9, 127.0.0.1")]);
        let from_proxy = resolve_origin(
            &h,
            Some("127.0.0.1:5000".parse().unwrap()),
            &trusted,
            Channel::Dashboard,
        );
        assert_eq!(from_proxy.ip.as_deref(), Some("203.0.113.9"));
        let spoofed = resolve_origin(
            &h,
            Some("198.51.100.1:5000".parse().unwrap()),
            &trusted,
            Channel::Dashboard,
        );
        assert_eq!(spoofed.ip.as_deref(), Some("198.51.100.1"));
        let none = resolve_origin(&h, None, &trusted, Channel::Dashboard);
        assert_eq!(none.ip, None);
        assert!(from_proxy.request_id.unwrap().starts_with("req_"));
    }

    #[test]
    fn same_origin_rules() {
        let ok = headers(&[
            ("origin", "http://localhost:9197"),
            ("host", "localhost:9197"),
        ]);
        assert!(same_origin(&ok, ""));
        let cross = headers(&[("origin", "http://evil.test"), ("host", "localhost:9197")]);
        assert!(!same_origin(&cross, ""));
        let public = headers(&[
            ("origin", "https://yanto.example.com"),
            ("host", "127.0.0.1:9197"),
        ]);
        assert!(same_origin(&public, "https://yanto.example.com"));
        assert!(!same_origin(&public, ""));
        let fetch = headers(&[
            ("origin", "http://a"),
            ("host", "a"),
            ("sec-fetch-site", "cross-site"),
        ]);
        assert!(!same_origin(&fetch, ""));
        let referer = headers(&[("referer", "http://a/page"), ("host", "a")]);
        assert!(same_origin(&referer, ""));
        assert!(
            !same_origin(&headers(&[("host", "a")]), ""),
            "no Origin/Referer is rejected"
        );
    }
}
