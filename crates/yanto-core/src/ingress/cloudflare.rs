//! Pure Cloudflare helpers: the full ingress list derived from Hostnames, drift comparison, and
//! the DNS targets (docs/rewrite/04 9.2).

use crate::ports::IngressRule;

/// Final rule required by cloudflared: the last ingress rule must be a catch-all.
pub const CATCH_ALL: &str = "http_status:404";
/// Comment Yanto puts on DNS records it creates (where the API supports it; ASSUMPTION).
pub const MANAGED_COMMENT: &str = "managed-by=yanto";

/// `(hostname, loopback port)` pairs to the full ingress list: sorted by hostname, then the
/// catch-all. The remote list is replaced as a whole on every reconcile.
pub fn build_ingress(hosts: &[(String, u16)]) -> Vec<IngressRule> {
    let mut sorted: Vec<&(String, u16)> = hosts.iter().collect();
    sorted.sort();
    let mut rules: Vec<IngressRule> = sorted
        .into_iter()
        .map(|(h, p)| IngressRule {
            hostname: Some(h.clone()),
            service: format!("http://127.0.0.1:{p}"),
        })
        .collect();
    rules.push(IngressRule {
        hostname: None,
        service: CATCH_ALL.to_owned(),
    });
    rules
}

/// `true` when the remote ingress differs from what the DB implies. Rules are compared as the
/// ordered list; the order Yanto pushes is deterministic.
pub fn drifted(expected: &[IngressRule], actual: &[IngressRule]) -> bool {
    expected != actual
}

/// CNAME target of a tunnel hostname.
pub fn cname_target(cf_tunnel_id: &str) -> String {
    format!("{cf_tunnel_id}.cfargotunnel.com")
}

/// The command the operator runs as root on the Host. Yanto only displays it.
pub fn service_install_command(token: &str) -> String {
    format!("cloudflared service install {token}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingress_is_sorted_and_ends_with_catch_all() {
        let r = build_ingress(&[("b.example.com".into(), 2), ("a.example.com".into(), 1)]);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].hostname.as_deref(), Some("a.example.com"));
        assert_eq!(r[0].service, "http://127.0.0.1:1");
        assert_eq!(r[2].hostname, None);
        assert_eq!(r[2].service, "http_status:404");
        assert_eq!(build_ingress(&[]).len(), 1);
    }

    #[test]
    fn drift_and_targets() {
        let a = build_ingress(&[("a.example.com".into(), 1)]);
        let mut b = a.clone();
        assert!(!drifted(&a, &b));
        b[0].service = "http://127.0.0.1:9".into();
        assert!(drifted(&a, &b));
        assert_eq!(cname_target("abc"), "abc.cfargotunnel.com");
        assert_eq!(
            service_install_command("T"),
            "cloudflared service install T"
        );
    }
}
