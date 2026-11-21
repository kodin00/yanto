//! Traefik file provider output (docs/rewrite/04 9.3): one Yanto-owned dynamic file generated from
//! a typed struct (never string concatenation), checked by parsing it back, plus a best-effort
//! check of the operator's static configuration.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{normalize_hostname, router_name};
use crate::error::CoreError;

pub const HEADER: &str = "# Managed by Yanto. Manual edits are overwritten.\n";

/// One Traefik Hostname: public name to a loopback port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub hostname: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSettings {
    pub entrypoint: String,
    pub cert_resolver: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DynFile {
    http: Http,
}

#[derive(Debug, Serialize, Deserialize)]
struct Http {
    routers: BTreeMap<String, Router>,
    services: BTreeMap<String, Service>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Router {
    rule: String,
    #[serde(rename = "entryPoints")]
    entry_points: Vec<String>,
    service: String,
    tls: Tls,
}

#[derive(Debug, Serialize, Deserialize)]
struct Tls {
    #[serde(rename = "certResolver")]
    cert_resolver: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Service {
    #[serde(rename = "loadBalancer")]
    load_balancer: LoadBalancer,
}

#[derive(Debug, Serialize, Deserialize)]
struct LoadBalancer {
    servers: Vec<Server>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Server {
    url: String,
}

fn simple_name(s: &str, what: &str) -> Result<(), CoreError> {
    let ok = !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'@'));
    if ok {
        Ok(())
    } else {
        Err(CoreError::validation(
            what.to_owned(),
            format!("{what} may only contain letters, digits, - _ . @"),
        ))
    }
}

/// Render `yanto.yml` for the given routes (sorted by hostname so output is stable) and verify
/// it by parsing it back. Routers use the TLS entry point only; no plaintext router is generated.
pub fn render(routes: &[Route], s: &RenderSettings) -> Result<String, CoreError> {
    simple_name(&s.entrypoint, "entrypoint")?;
    simple_name(&s.cert_resolver, "cert_resolver")?;
    let mut sorted: Vec<&Route> = routes.iter().collect();
    sorted.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    let mut http = Http {
        routers: BTreeMap::new(),
        services: BTreeMap::new(),
    };
    for r in sorted {
        // Defense in depth: the service validated already, the rule text must stay safe.
        if normalize_hostname(&r.hostname)? != r.hostname {
            return Err(CoreError::validation(
                "hostname",
                "hostname is not normalized",
            ));
        }
        let name = router_name(&r.hostname);
        if http.routers.contains_key(&name) {
            return Err(CoreError::conflict(format!(
                "router name {name} collides with another Hostname"
            )));
        }
        http.routers.insert(
            name.clone(),
            Router {
                rule: format!("Host(`{}`)", r.hostname),
                entry_points: vec![s.entrypoint.clone()],
                service: name.clone(),
                tls: Tls {
                    cert_resolver: s.cert_resolver.clone(),
                },
            },
        );
        http.services.insert(
            name,
            Service {
                load_balancer: LoadBalancer {
                    servers: vec![Server {
                        url: format!("http://127.0.0.1:{}", r.port),
                    }],
                },
            },
        );
    }
    let body = serde_norway::to_string(&DynFile { http })
        .map_err(|e| CoreError::internal("cannot render traefik file", e))?;
    let out = format!("{HEADER}{body}");
    verify(&out, routes)?;
    Ok(out)
}

/// Invariants checked before a write: every router references an existing service, every URL is
/// `http://127.0.0.1:<declared port>`, and the set of hostnames equals `routes`.
pub fn verify(content: &str, routes: &[Route]) -> Result<(), CoreError> {
    let bad = |m: String| CoreError::new(crate::ErrorKind::Internal, m);
    let f: DynFile = serde_norway::from_str(content)
        .map_err(|e| CoreError::internal("generated traefik file does not parse", e))?;
    if f.http.routers.len() != routes.len() {
        return Err(bad("router count does not match Hostnames".into()));
    }
    for r in routes {
        let name = router_name(&r.hostname);
        let router = f
            .http
            .routers
            .get(&name)
            .ok_or_else(|| bad(format!("router {name} missing")))?;
        if router.rule != format!("Host(`{}`)", r.hostname) {
            return Err(bad(format!("router {name} has an unexpected rule")));
        }
        let svc = f
            .http
            .services
            .get(&router.service)
            .ok_or_else(|| bad(format!("router {name} references a missing service")))?;
        let want = format!("http://127.0.0.1:{}", r.port);
        if svc.load_balancer.servers.len() != 1 || svc.load_balancer.servers[0].url != want {
            return Err(bad(format!("service {name} does not point at {want}")));
        }
    }
    Ok(())
}

// ---- static configuration prerequisites ---------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    /// Static config passed by flags or environment only; not a failure (04 9.3).
    Unverifiable,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckItem {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
}

impl CheckItem {
    fn new(name: &'static str, status: CheckStatus, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }
}

/// Case-insensitive object key lookup (Traefik's loaders are case-insensitive).
fn get_ci<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

fn parse_static(path: &Path, text: &str) -> Result<Value, String> {
    if path.extension().is_some_and(|e| e == "toml") {
        toml::from_str::<Value>(text).map_err(|e| e.to_string())
    } else {
        serde_norway::from_str::<Value>(text).map_err(|e| e.to_string())
    }
}

/// Checklist for the static configuration: file provider directory equals `dynamic_dir`,
/// `watch: true`, the entry point and the certificate resolver exist.
pub fn check_static(
    path: &Path,
    text: Option<&str>,
    dynamic_dir: &Path,
    entrypoint: &str,
    resolver: &str,
) -> Vec<CheckItem> {
    const NAMES: [&str; 4] = [
        "file provider directory",
        "file provider watch",
        "entry point",
        "certificate resolver",
    ];
    let unverifiable = |why: String| -> Vec<CheckItem> {
        NAMES
            .iter()
            .map(|n| CheckItem::new(n, CheckStatus::Unverifiable, why.clone()))
            .collect()
    };
    let Some(text) = text else {
        return unverifiable(format!(
            "static config {} not found (flags or environment only?)",
            path.display()
        ));
    };
    let root = match parse_static(path, text) {
        Ok(v) => v,
        Err(e) => return unverifiable(format!("cannot parse {}: {e}", path.display())),
    };
    let file = get_ci(&root, "providers").and_then(|p| get_ci(p, "file"));
    let mut out = Vec::new();
    let dir = file
        .and_then(|f| get_ci(f, "directory"))
        .and_then(Value::as_str);
    out.push(match dir {
        Some(d) if Path::new(d) == dynamic_dir => {
            CheckItem::new(NAMES[0], CheckStatus::Pass, d.to_owned())
        }
        Some(d) => CheckItem::new(
            NAMES[0],
            CheckStatus::Fail,
            format!(
                "file provider loads {d}, Yanto writes to {}",
                dynamic_dir.display()
            ),
        ),
        None => CheckItem::new(
            NAMES[0],
            CheckStatus::Fail,
            format!(
                "no providers.file.directory; set it to {}",
                dynamic_dir.display()
            ),
        ),
    });
    let watch = file
        .and_then(|f| get_ci(f, "watch"))
        .and_then(Value::as_bool);
    out.push(match watch {
        Some(true) => CheckItem::new(NAMES[1], CheckStatus::Pass, "watch: true"),
        _ => CheckItem::new(
            NAMES[1],
            CheckStatus::Fail,
            "set providers.file.watch to true so Traefik reloads without a restart",
        ),
    });
    let ep = get_ci(&root, "entryPoints").and_then(|e| get_ci(e, entrypoint));
    out.push(if ep.is_some() {
        CheckItem::new(NAMES[2], CheckStatus::Pass, entrypoint.to_owned())
    } else {
        CheckItem::new(
            NAMES[2],
            CheckStatus::Fail,
            format!("entry point {entrypoint} is not defined"),
        )
    });
    let cr = get_ci(&root, "certificatesResolvers").and_then(|e| get_ci(e, resolver));
    out.push(if cr.is_some() {
        CheckItem::new(NAMES[3], CheckStatus::Pass, resolver.to_owned())
    } else {
        CheckItem::new(
            NAMES[3],
            CheckStatus::Fail,
            format!("certificate resolver {resolver} is not defined"),
        )
    });
    out
}

/// Static configuration the operator must provide (shown in the UI).
pub fn prerequisites_yaml(dynamic_dir: &Path, entrypoint: &str, resolver: &str) -> String {
    format!(
        "entryPoints:
  web:
    address: \":80\"
  {entrypoint}:
    address: \":443\"
providers:
  file:
    directory: {dir}
    watch: true
certificatesResolvers:
  {resolver}:
    acme:
      email: <operator email>
      storage: /etc/traefik/acme.json
      httpChallenge:
        entryPoint: web
",
        dir = dynamic_dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> RenderSettings {
        RenderSettings {
            entrypoint: "websecure".into(),
            cert_resolver: "letsencrypt".into(),
        }
    }

    #[test]
    fn renders_documented_shape_and_is_stable() {
        let routes = vec![
            Route {
                hostname: "b.example.com".into(),
                port: 8081,
            },
            Route {
                hostname: "app.example.com".into(),
                port: 8080,
            },
        ];
        let a = render(&routes, &settings()).unwrap();
        let b = render(&[routes[1].clone(), routes[0].clone()], &settings()).unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with(HEADER));
        assert!(a.contains("Host(`app.example.com`)"));
        assert!(a.contains("certResolver: letsencrypt"));
        assert!(a.contains("http://127.0.0.1:8080"));
        assert!(a.contains("yanto-app-example-com"));
        assert!(a.contains("entryPoints"));
        assert!(!a.contains("web\n"), "no plaintext router");
    }

    #[test]
    fn empty_set_renders_and_verifies() {
        let a = render(&[], &settings()).unwrap();
        verify(&a, &[]).unwrap();
    }

    #[test]
    fn injection_in_hostname_is_refused() {
        let r = Route {
            hostname: "x.example.com`) || Host(`y.example.com".into(),
            port: 1,
        };
        assert!(render(&[r], &settings()).is_err());
        let r = Route {
            hostname: "A.example.com".into(),
            port: 1,
        };
        assert!(render(&[r], &settings()).is_err());
        let mut s = settings();
        s.cert_resolver = "a\nb: c".into();
        assert!(render(&[], &s).is_err());
    }

    #[test]
    fn verify_catches_tampering() {
        let routes = vec![Route {
            hostname: "a.example.com".into(),
            port: 8080,
        }];
        let ok = render(&routes, &settings()).unwrap();
        let bad = ok.replace("127.0.0.1:8080", "10.0.0.1:8080");
        assert!(verify(&bad, &routes).is_err());
        assert!(verify(&ok, &[]).is_err());
    }

    const STATIC_OK: &str = "entryPoints:\n  web:\n    address: ':80'\n  websecure:\n    address: ':443'\nproviders:\n  file:\n    directory: /etc/traefik/dynamic\n    watch: true\ncertificatesResolvers:\n  letsencrypt:\n    acme:\n      email: a@b.c\n      storage: /x\n";

    fn run(text: Option<&str>) -> Vec<CheckItem> {
        check_static(
            Path::new("/etc/traefik/traefik.yml"),
            text,
            Path::new("/etc/traefik/dynamic"),
            "websecure",
            "letsencrypt",
        )
    }

    #[test]
    fn static_config_checklist() {
        assert!(
            run(Some(STATIC_OK))
                .iter()
                .all(|c| c.status == CheckStatus::Pass)
        );
        let c = run(Some(&STATIC_OK.replace("watch: true", "watch: false")));
        assert_eq!(c[1].status, CheckStatus::Fail);
        let c = run(Some(&STATIC_OK.replace("letsencrypt:", "other:")));
        assert_eq!(c[3].status, CheckStatus::Fail);
        let c = run(Some(&STATIC_OK.replace("websecure:", "secure:")));
        assert_eq!(c[2].status, CheckStatus::Fail);
        let c = run(Some("entryPoints: {}\n"));
        assert_eq!(c[0].status, CheckStatus::Fail);
        let c = run(Some(&STATIC_OK.replace("/etc/traefik/dynamic", "/other")));
        assert_eq!(c[0].status, CheckStatus::Fail);
    }

    #[test]
    fn missing_or_broken_static_config_is_unverifiable() {
        assert!(
            run(None)
                .iter()
                .all(|c| c.status == CheckStatus::Unverifiable)
        );
        assert!(
            run(Some(": : :\n\t"))
                .iter()
                .all(|c| c.status == CheckStatus::Unverifiable)
        );
    }

    #[test]
    fn toml_static_config_works() {
        let t = "[entryPoints.websecure]\naddress = ':443'\n[providers.file]\ndirectory = '/etc/traefik/dynamic'\nwatch = true\n[certificatesResolvers.letsencrypt.acme]\nemail = 'a@b.c'\n";
        let c = check_static(
            Path::new("/etc/traefik/traefik.toml"),
            Some(t),
            Path::new("/etc/traefik/dynamic"),
            "websecure",
            "letsencrypt",
        );
        assert!(c.iter().all(|c| c.status == CheckStatus::Pass), "{c:?}");
    }
}
