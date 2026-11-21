//! Compose helpers: published-port derivation from `docker compose config --format json`,
//! loopback enforcement, the collision check, and image/registry helpers.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::error::CoreError;
use crate::ports::Listener;

/// A host port a Project publishes, as derived from its Compose file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DesiredPort {
    pub label: String,
    pub service: String,
    pub host_port: u16,
    pub container_port: u16,
    pub bind_address: String,
    pub protocol: String,
}

const RANGE_CAP: u32 = 512;

/// Derive published ports. Fails when a mapping is not published on a loopback address
/// (04 1.1: "publish on 127.0.0.1 so an Ingress Provider carries the traffic").
pub fn ports_from_config(config: &Value) -> Result<Vec<DesiredPort>, CoreError> {
    let mut out = Vec::new();
    let mut offenders = Vec::new();
    let empty = serde_json::Map::new();
    let services = config
        .get("services")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for (svc, def) in services {
        let Some(ports) = def.get("ports").and_then(Value::as_array) else {
            continue;
        };
        for p in ports {
            let Some(published) = p.get("published").and_then(value_text) else {
                continue; // container-only port, nothing on the Host
            };
            if published.is_empty() {
                continue;
            }
            let target = p.get("target").and_then(Value::as_u64).unwrap_or(0);
            let protocol = p
                .get("protocol")
                .and_then(Value::as_str)
                .unwrap_or("tcp")
                .to_owned();
            let host_ip = p.get("host_ip").and_then(Value::as_str).unwrap_or("");
            if !matches!(host_ip, "127.0.0.1" | "::1") {
                offenders.push(format!("{svc}: {published}->{target}"));
                continue;
            }
            for host_port in expand_range(&published)? {
                let container_port = u16::try_from(target).map_err(|_| {
                    CoreError::validation("compose", format!("invalid container port in {svc}"))
                })?;
                out.push(DesiredPort {
                    label: String::new(),
                    service: svc.clone(),
                    host_port,
                    container_port,
                    bind_address: host_ip.to_owned(),
                    protocol: protocol.clone(),
                });
            }
        }
    }
    if !offenders.is_empty() {
        return Err(CoreError::validation(
            "compose",
            format!(
                "publish on 127.0.0.1 so an Ingress Provider carries the traffic; not loopback: {}",
                offenders.join(", ")
            ),
        ));
    }
    out.sort();
    assign_labels(&mut out);
    Ok(out)
}

fn value_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn expand_range(s: &str) -> Result<Vec<u16>, CoreError> {
    let bad = || CoreError::validation("compose", format!("invalid published port {s:?}"));
    match s.split_once('-') {
        None => Ok(vec![s.parse::<u16>().map_err(|_| bad())?]),
        Some((a, b)) => {
            let (a, b): (u16, u16) = (a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?);
            if b < a || u32::from(b - a) + 1 > RANGE_CAP {
                return Err(CoreError::validation(
                    "compose",
                    format!("port range {s} is empty or larger than {RANGE_CAP}"),
                ));
            }
            Ok((a..=b).collect())
        }
    }
}

/// Labels: the service name, or `<service>-<container port>` when a service has several.
fn assign_labels(ports: &mut [DesiredPort]) {
    let mut per_service: BTreeMap<String, usize> = BTreeMap::new();
    for p in ports.iter() {
        *per_service.entry(p.service.clone()).or_default() += 1;
    }
    let mut used = BTreeSet::new();
    for p in ports.iter_mut() {
        let mut label = if per_service[&p.service] == 1 {
            p.service.clone()
        } else {
            format!("{}-{}", p.service, p.container_port)
        };
        if !used.insert(label.clone()) {
            label = format!("{}-{}", label, p.host_port);
            used.insert(label.clone());
        }
        label.truncate(64);
        p.label = label;
    }
}

/// Images named by the services of a `compose config` document.
pub fn images_from_config(config: &Value) -> Vec<String> {
    config
        .get("services")
        .and_then(Value::as_object)
        .map(|s| {
            s.values()
                .filter_map(|d| d.get("image").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Generated compose for an image Project without pasted compose content.
pub fn generated_image_compose(image_ref: &str) -> String {
    format!("services:\n  app:\n    image: {image_ref}\n")
}

/// Restart-policy override written when `auto_start` is on (04 1.3 step 8).
pub fn restart_override(services: &[String]) -> String {
    let mut s = String::from("services:\n");
    for svc in services {
        s.push_str(&format!("  {svc}:\n    restart: unless-stopped\n"));
    }
    s
}

/// The registry host of an image reference (`docker.io` when none is named).
pub fn registry_host(image_ref: &str) -> String {
    let name = image_ref.split('@').next().unwrap_or(image_ref);
    match name.split_once('/') {
        Some((first, _)) if first.contains('.') || first.contains(':') || first == "localhost" => {
            first.to_owned()
        }
        _ => "docker.io".to_owned(),
    }
}

/// Key under `auths` in a Docker `config.json` for a registry host.
pub fn auth_key(host: &str) -> String {
    match host.trim().to_ascii_lowercase().as_str() {
        "docker.io" | "index.docker.io" | "registry-1.docker.io" | "registry.hub.docker.com" => {
            "https://index.docker.io/v1/".to_owned()
        }
        other => other.to_owned(),
    }
}

/// A published port of a running container, from `docker ps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPort {
    pub container: String,
    pub compose_project: String,
    pub host_port: u16,
    pub protocol: String,
}

/// Parse lines of `name<TAB>compose project<TAB>docker ps Ports column`.
pub fn parse_ps_ports(output: &str) -> Vec<ContainerPort> {
    let mut out = Vec::new();
    for line in output.lines() {
        let mut cols = line.splitn(3, '\t');
        let (Some(name), Some(project), Some(ports)) = (cols.next(), cols.next(), cols.next())
        else {
            continue;
        };
        for entry in ports.split(',') {
            let entry = entry.trim();
            let Some((host, rest)) = entry.split_once("->") else {
                continue;
            };
            let protocol = rest.rsplit('/').next().unwrap_or("tcp");
            let Some(port_part) = host.rsplit(':').next() else {
                continue;
            };
            let range = match port_part.split_once('-') {
                Some((a, b)) => a.parse::<u16>().ok().zip(b.parse::<u16>().ok()),
                None => port_part.parse::<u16>().ok().map(|p| (p, p)),
            };
            if let Some((a, b)) = range {
                for p in a..=b.min(a.saturating_add(511)) {
                    out.push(ContainerPort {
                        container: name.to_owned(),
                        compose_project: project.to_owned(),
                        host_port: p,
                        protocol: protocol.to_owned(),
                    });
                }
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub host_port: u16,
    pub protocol: String,
    pub owner: String,
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{} is used by {}",
            self.host_port, self.protocol, self.owner
        )
    }
}

/// Everything the collision check compares against (docs/rewrite/04 1.1).
pub struct CollisionInputs<'a> {
    pub slug: &'a str,
    pub desired: &'a [DesiredPort],
    /// (other Project name, host port, protocol) declared by other Projects.
    pub other_projects: &'a [(String, u16, String)],
    pub containers: &'a [ContainerPort],
    pub listeners: &'a [Listener],
}

/// Up to 8 conflicts, each naming its owner.
pub fn find_conflicts(i: &CollisionInputs<'_>) -> Vec<Conflict> {
    let mut out: Vec<Conflict> = Vec::new();
    let mut seen_in_project = BTreeSet::new();
    let own_published: BTreeSet<(u16, &str)> = i
        .containers
        .iter()
        .filter(|c| c.compose_project == i.slug)
        .map(|c| (c.host_port, c.protocol.as_str()))
        .collect();
    for d in i.desired {
        let key = (d.host_port, d.protocol.clone());
        let mut push = |owner: String| {
            if !out
                .iter()
                .any(|c| c.host_port == d.host_port && c.protocol == d.protocol && c.owner == owner)
            {
                out.push(Conflict {
                    host_port: d.host_port,
                    protocol: d.protocol.clone(),
                    owner,
                });
            }
        };
        if !seen_in_project.insert(key) {
            push("this project (declared twice)".into());
        }
        for (name, port, proto) in i.other_projects {
            if *port == d.host_port && *proto == d.protocol {
                push(format!("project {name}"));
            }
        }
        for c in i.containers {
            if c.compose_project != i.slug && c.host_port == d.host_port && c.protocol == d.protocol
            {
                push(format!("container {}", c.container));
            }
        }
        if !own_published.contains(&(d.host_port, d.protocol.as_str())) {
            for l in i.listeners {
                if l.port == d.host_port && l.protocol == d.protocol {
                    push(match &l.process {
                        Some(p) => format!("host process {p}"),
                        None => "a host process".into(),
                    });
                }
            }
        }
    }
    out.truncate(8);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(ports: Value) -> Value {
        json!({"services": {"web": {"image": "nginx:alpine", "ports": ports}}})
    }

    #[test]
    fn loopback_ports_are_derived() {
        let got = ports_from_config(&cfg(json!([
            {"mode":"ingress","host_ip":"127.0.0.1","target":80,"published":"8080","protocol":"tcp"}
        ])))
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            (
                got[0].label.as_str(),
                got[0].host_port,
                got[0].container_port
            ),
            ("web", 8080, 80)
        );
    }

    #[test]
    fn public_or_bare_publishing_is_rejected() {
        for ip in [json!("0.0.0.0"), json!(null)] {
            let e = ports_from_config(&cfg(json!([
                {"host_ip": ip, "target": 80, "published": "8080", "protocol": "tcp"}
            ])))
            .unwrap_err();
            assert!(e.message.contains("127.0.0.1"), "{}", e.message);
        }
        // Container-only ports are fine.
        assert!(
            ports_from_config(&cfg(json!([{"target": 80, "protocol": "tcp"}])))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn ranges_expand_and_are_capped() {
        let got = ports_from_config(&cfg(json!([
            {"host_ip":"127.0.0.1","target":80,"published":"9000-9002","protocol":"tcp"}
        ])))
        .unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(
            got.iter()
                .map(|p| p.label.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            3,
            "labels stay unique"
        );
        assert!(
            ports_from_config(&cfg(json!([
                {"host_ip":"127.0.0.1","target":80,"published":"1000-2000","protocol":"tcp"}
            ])))
            .is_err()
        );
    }

    #[test]
    fn registry_hosts() {
        assert_eq!(registry_host("nginx:alpine"), "docker.io");
        assert_eq!(registry_host("org/app"), "docker.io");
        assert_eq!(registry_host("ghcr.io/org/app:1"), "ghcr.io");
        assert_eq!(registry_host("127.0.0.1:5000/app"), "127.0.0.1:5000");
        assert_eq!(registry_host("localhost/app"), "localhost");
        assert_eq!(auth_key("docker.io"), "https://index.docker.io/v1/");
        assert_eq!(auth_key("GHCR.io"), "ghcr.io");
    }

    #[test]
    fn docker_ps_ports_parse() {
        let out = "a\tother\t0.0.0.0:8080->80/tcp, :::8080->80/tcp, 443/tcp\nb\tp\t127.0.0.1:9000-9001->9000-9001/udp\nbad line\n";
        let got = parse_ps_ports(out);
        assert_eq!(got.iter().filter(|c| c.container == "a").count(), 2);
        assert!(
            got.iter()
                .any(|c| c.container == "b" && c.host_port == 9001 && c.protocol == "udp")
        );
    }

    fn desired(port: u16) -> DesiredPort {
        DesiredPort {
            label: "web".into(),
            service: "web".into(),
            host_port: port,
            container_port: 80,
            bind_address: "127.0.0.1".into(),
            protocol: "tcp".into(),
        }
    }

    #[test]
    fn collisions_cover_each_source_and_never_self() {
        let containers = vec![
            ContainerPort {
                container: "foreign".into(),
                compose_project: "x".into(),
                host_port: 8081,
                protocol: "tcp".into(),
            },
            ContainerPort {
                container: "mine".into(),
                compose_project: "me".into(),
                host_port: 8080,
                protocol: "tcp".into(),
            },
        ];
        let listeners = vec![
            Listener {
                port: 8082,
                protocol: "tcp".into(),
                process: Some("nginx".into()),
            },
            // docker-proxy of this Project's own running container: not a conflict.
            Listener {
                port: 8080,
                protocol: "tcp".into(),
                process: Some("docker-proxy".into()),
            },
        ];
        let others = vec![("Other".to_owned(), 8083u16, "tcp".to_owned())];
        let dp = [
            desired(8080),
            desired(8081),
            desired(8082),
            desired(8083),
            desired(8083),
        ];
        let got = find_conflicts(&CollisionInputs {
            slug: "me",
            desired: &dp,
            other_projects: &others,
            containers: &containers,
            listeners: &listeners,
        });
        let owners: Vec<&str> = got.iter().map(|c| c.owner.as_str()).collect();
        assert!(owners.contains(&"container foreign"));
        assert!(owners.contains(&"host process nginx"));
        assert!(owners.contains(&"project Other"));
        assert!(owners.contains(&"this project (declared twice)"));
        assert!(
            !got.iter().any(|c| c.host_port == 8080),
            "no self collision: {got:?}"
        );
    }

    #[test]
    fn restart_override_lists_services() {
        let s = restart_override(&["a".into(), "b".into()]);
        assert!(s.contains("  a:\n    restart: unless-stopped"));
    }
}
