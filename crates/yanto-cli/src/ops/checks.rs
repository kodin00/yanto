//! Prerequisite and Host Daemon checks (docs/rewrite/05 sections 3.1, 3.3).
//! Checks only report; they never install anything (D15).

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use super::host::Host;
use super::layout::{Layout, OS_RELEASE, SYSTEMD_RUN_DIR};

pub const MIN_FREE_BYTES: u64 = 2 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
    pub hint: Option<String>,
}

impl Check {
    fn new(name: &str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
            hint: None,
        }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Inputs the prerequisite checks depend on.
#[derive(Debug, Clone)]
pub struct PrereqInput {
    pub bind: IpAddr,
    pub port: u16,
    /// Skip the port-free check (port belongs to an existing Yanto install).
    pub port_is_ours: bool,
    pub data_dir: PathBuf,
    pub docker_socket: PathBuf,
    pub registry: bool,
}

pub fn worst(checks: &[Check]) -> Status {
    checks.iter().map(|c| c.status).max().unwrap_or(Status::Ok)
}

pub fn prereq_checks(host: &dyn Host, layout: &Layout, input: &PrereqInput) -> Vec<Check> {
    let mut out = vec![
        check_root(host),
        check_systemd(host, layout),
        check_distro(layout),
        check_docker(host, &input.docker_socket),
        check_compose(host),
        check_git(host),
        check_port(host, input),
        check_disk(host, &input.data_dir),
    ];
    if input.registry {
        out.push(check_registry_port(host));
    }
    out
}

fn check_root(host: &dyn Host) -> Check {
    if host.euid() == 0 {
        Check::new("root", Status::Ok, "running as root")
    } else {
        Check::new("root", Status::Fail, "not running as root")
            .hint("re-run with sudo: sudo yanto install")
    }
}

fn check_systemd(host: &dyn Host, layout: &Layout) -> Check {
    let present = layout.path(SYSTEMD_RUN_DIR).exists();
    if !present {
        return Check::new(
            "systemd",
            Status::Fail,
            format!("{SYSTEMD_RUN_DIR} missing"),
        )
        .hint("Yanto requires Linux with systemd as PID 1");
    }
    match host.run("systemctl", &["--version"]) {
        Ok(o) if o.success => Check::new(
            "systemd",
            Status::Ok,
            o.stdout.lines().next().unwrap_or("systemd").trim(),
        ),
        _ => Check::new("systemd", Status::Fail, "systemctl not usable")
            .hint("install/enable systemd; Yanto cannot run without it"),
    }
}

/// Parse `KEY=value` lines of os-release (values may be quoted).
pub fn parse_os_release(text: &str) -> std::collections::HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            Some((
                k.trim().to_string(),
                v.trim().trim_matches('"').trim_matches('\'').to_string(),
            ))
        })
        .collect()
}

/// `Some(true)` when the distro is a tested one (Debian >= 12, Ubuntu >= 22.04).
pub fn distro_supported(id: &str, version_id: &str) -> bool {
    let mut parts = version_id.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    match id {
        "debian" => major >= 12,
        "ubuntu" => (major, minor) >= (22, 4),
        _ => false,
    }
}

fn check_distro(layout: &Layout) -> Check {
    let Ok(text) = std::fs::read_to_string(layout.path(OS_RELEASE)) else {
        return Check::new("distro", Status::Warn, "cannot read /etc/os-release")
            .hint("tested on Debian 12+ and Ubuntu 22.04+");
    };
    let kv = parse_os_release(&text);
    let id = kv.get("ID").map_or("unknown", String::as_str);
    let ver = kv.get("VERSION_ID").map_or("", String::as_str);
    let label = format!("{id} {ver}");
    if distro_supported(id, ver) {
        Check::new("distro", Status::Ok, label)
    } else {
        Check::new(
            "distro",
            Status::Warn,
            format!("{label} is not a tested distro"),
        )
        .hint("tested on Debian 12+ and Ubuntu 22.04+; continuing is at your own risk")
    }
}

const DOCKER_HINT: &str = "install Docker Engine from the official instructions for your distro: https://docs.docker.com/engine/install/ (Yanto never installs it)";

fn check_docker(host: &dyn Host, socket: &Path) -> Check {
    if host.which("docker").is_none() {
        return Check::new("docker", Status::Fail, "docker CLI not found on PATH")
            .hint(DOCKER_HINT);
    }
    let sock = format!("unix://{}", socket.display());
    match host.run(
        "docker",
        &["--host", &sock, "info", "--format", "{{.ServerVersion}}"],
    ) {
        Ok(o) if o.success => Check::new(
            "docker",
            Status::Ok,
            format!("daemon reachable, server {}", o.stdout.trim()),
        ),
        _ => Check::new(
            "docker",
            Status::Fail,
            format!("daemon not reachable on {}", socket.display()),
        )
        .hint("start it: systemctl enable --now docker; or fix docker.socket in config"),
    }
}

fn check_compose(host: &dyn Host) -> Check {
    match host.run("docker", &["compose", "version"]) {
        Ok(o) if o.success => Check::new("compose", Status::Ok, o.stdout.trim()),
        _ => Check::new("compose", Status::Fail, "docker compose plugin not available").hint(
            "install the Compose v2 plugin (docker-compose-plugin): https://docs.docker.com/compose/install/linux/",
        ),
    }
}

fn check_git(host: &dyn Host) -> Check {
    match host.run("git", &["--version"]) {
        Ok(o) if o.success => Check::new("git", Status::Ok, o.stdout.trim()),
        _ => Check::new("git", Status::Fail, "git not found")
            .hint("Debian/Ubuntu: apt-get install git (needed for Git-sourced Projects)"),
    }
}

fn check_port(host: &dyn Host, input: &PrereqInput) -> Check {
    let name = "port";
    let addr = format!("{}:{}", input.bind, input.port);
    if input.port_is_ours {
        return Check::new(
            name,
            Status::Ok,
            format!("{addr} (existing Yanto install, not probed)"),
        );
    }
    if host.port_free(input.bind, input.port) {
        Check::new(name, Status::Ok, format!("{addr} is free"))
    } else {
        Check::new(name, Status::Fail, format!("{addr} is already in use"))
            .hint("pick another port with --port, or stop the process using it")
    }
}

fn check_registry_port(host: &dyn Host) -> Check {
    let lo = IpAddr::from([127, 0, 0, 1]);
    if host.port_free(lo, 5000) {
        Check::new("registry port", Status::Ok, "127.0.0.1:5000 is free")
    } else {
        Check::new("registry port", Status::Warn, "127.0.0.1:5000 is in use")
            .hint("the Hosted Registry needs 127.0.0.1:5000 when the Owner enables it")
    }
}

fn check_disk(host: &dyn Host, data_dir: &Path) -> Check {
    match host.free_bytes(data_dir) {
        Some(b) if b >= MIN_FREE_BYTES => {
            Check::new("disk", Status::Ok, format!("{} GiB free", b >> 30))
        }
        Some(b) => Check::new("disk", Status::Warn, format!("only {} MiB free", b >> 20))
            .hint("at least 2 GiB free is recommended on the data dir's filesystem"),
        None => Check::new("disk", Status::Warn, "cannot determine free space"),
    }
}

/// Which Host Daemons to look for.
#[derive(Debug, Clone, Default)]
pub struct DaemonInput {
    pub cloudflared_unit: Option<String>,
    pub traefik_unit: Option<String>,
    pub frps_unit: Option<String>,
}

/// Binary, unit presence, enabled, active for each expected daemon. Always
/// warn-level at worst: the operator may install them later (D15).
pub fn daemon_checks(host: &dyn Host, input: &DaemonInput) -> Vec<Check> {
    [
        ("cloudflared", &input.cloudflared_unit),
        ("traefik", &input.traefik_unit),
        ("frps", &input.frps_unit),
    ]
    .into_iter()
    .filter_map(|(bin, unit)| unit.as_deref().map(|u| daemon_check(host, bin, u)))
    .collect()
}

fn daemon_check(host: &dyn Host, bin: &str, unit: &str) -> Check {
    let name = format!("daemon {bin}");
    if host.which(bin).is_none() {
        return Check::new(&name, Status::Warn, "binary not found on PATH").hint(format!(
            "install {bin} yourself; Yanto never installs Host Daemons"
        ));
    }
    let unit_file = format!("{unit}.service");
    if !host.succeeds("systemctl", &["cat", &unit_file]) {
        return Check::new(&name, Status::Warn, format!("unit {unit_file} not found")).hint(
            format!("create the {unit_file} systemd unit, or set the unit name in config"),
        );
    }
    let enabled = host.succeeds("systemctl", &["is-enabled", "--quiet", &unit_file]);
    let active = host.succeeds("systemctl", &["is-active", "--quiet", &unit_file]);
    let detail = format!(
        "unit {unit_file}: {}, {}",
        if enabled { "enabled" } else { "not enabled" },
        if active { "active" } else { "not active" }
    );
    if enabled && active {
        Check::new(&name, Status::Ok, detail)
    } else {
        Check::new(&name, Status::Warn, detail).hint(format!("systemctl enable --now {unit_file}"))
    }
}

/// Render checks as an aligned table, hints on following lines.
pub fn render_table(checks: &[Check]) -> String {
    let w = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut s = String::new();
    for c in checks {
        s.push_str(&format!(
            "  {:<4}  {:<w$}  {}\n",
            c.status.label(),
            c.name,
            c.detail,
            w = w
        ));
        if c.status != Status::Ok
            && let Some(h) = &c.hint
        {
            s.push_str(&format!("        {:<w$}  hint: {h}\n", "", w = w));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::host::{CmdOutput, FakeHost};

    fn input() -> PrereqInput {
        PrereqInput {
            bind: "127.0.0.1".parse().unwrap(),
            port: 9197,
            port_is_ours: false,
            data_dir: "/var/lib/yanto".into(),
            docker_socket: "/var/run/docker.sock".into(),
            registry: false,
        }
    }

    fn layout_with(os_release: &str, systemd: bool) -> (tempfile::TempDir, Layout) {
        let dir = tempfile::tempdir().unwrap();
        let l = Layout::new(dir.path());
        std::fs::create_dir_all(l.path("/etc")).unwrap();
        std::fs::write(l.path(OS_RELEASE), os_release).unwrap();
        if systemd {
            std::fs::create_dir_all(l.path(SYSTEMD_RUN_DIR)).unwrap();
        }
        (dir, l)
    }

    fn by_name<'a>(c: &'a [Check], n: &str) -> &'a Check {
        c.iter().find(|c| c.name == n).unwrap()
    }

    #[test]
    fn all_good_is_all_ok() {
        let (_d, l) = layout_with("ID=debian\nVERSION_ID=\"12\"\n", true);
        let checks = prereq_checks(&FakeHost::healthy_host(), &l, &input());
        assert_eq!(worst(&checks), Status::Ok, "{}", render_table(&checks));
    }

    #[test]
    fn non_root_fails() {
        let (_d, l) = layout_with("ID=debian\nVERSION_ID=\"12\"\n", true);
        let mut h = FakeHost::healthy_host();
        h.euid = 1000;
        let c = prereq_checks(&h, &l, &input());
        assert_eq!(by_name(&c, "root").status, Status::Fail);
    }

    #[test]
    fn missing_docker_compose_git_fail_with_hints_and_never_install() {
        let (_d, l) = layout_with("ID=ubuntu\nVERSION_ID=\"22.04\"\n", true);
        let h = FakeHost::default();
        let c = prereq_checks(&h, &l, &input());
        for n in ["docker", "compose", "git", "systemd"] {
            assert_eq!(by_name(&c, n).status, Status::Fail, "{n}");
        }
        assert!(by_name(&c, "docker").hint.is_some());
        assert!(
            h.calls.borrow().iter().all(|c| !c.contains("install")),
            "checks must not install anything"
        );
    }

    #[test]
    fn docker_daemon_down_fails_even_with_cli() {
        let (_d, l) = layout_with("ID=debian\nVERSION_ID=\"12\"\n", true);
        let mut h = FakeHost::healthy_host();
        h.respond(
            "docker --host unix:///var/run/docker.sock info --format {{.ServerVersion}}",
            CmdOutput::fail(),
        );
        let c = prereq_checks(&h, &l, &input());
        assert_eq!(by_name(&c, "docker").status, Status::Fail);
    }

    #[test]
    fn busy_port_fails_unless_ours() {
        let (_d, l) = layout_with("ID=debian\nVERSION_ID=\"12\"\n", true);
        let mut h = FakeHost::healthy_host();
        h.busy_ports.insert(9197);
        assert_eq!(
            by_name(&prereq_checks(&h, &l, &input()), "port").status,
            Status::Fail
        );
        let mut i = input();
        i.port_is_ours = true;
        assert_eq!(
            by_name(&prereq_checks(&h, &l, &i), "port").status,
            Status::Ok
        );
    }

    #[test]
    fn distro_matrix() {
        assert!(distro_supported("debian", "12"));
        assert!(!distro_supported("debian", "11"));
        assert!(distro_supported("ubuntu", "22.04"));
        assert!(distro_supported("ubuntu", "24.04"));
        assert!(!distro_supported("ubuntu", "20.04"));
        assert!(!distro_supported("fedora", "40"));
    }

    #[test]
    fn untested_distro_warns_and_low_disk_warns() {
        let (_d, l) = layout_with("ID=fedora\nVERSION_ID=40\n", true);
        let mut h = FakeHost::healthy_host();
        h.free = Some(100 << 20);
        let c = prereq_checks(&h, &l, &input());
        assert_eq!(by_name(&c, "distro").status, Status::Warn);
        assert_eq!(by_name(&c, "disk").status, Status::Warn);
        assert_eq!(worst(&c), Status::Warn);
    }

    #[test]
    fn registry_port_checked_only_when_enabled() {
        let (_d, l) = layout_with("ID=debian\nVERSION_ID=\"12\"\n", true);
        let mut h = FakeHost::healthy_host();
        h.busy_ports.insert(5000);
        assert!(
            !prereq_checks(&h, &l, &input())
                .iter()
                .any(|c| c.name == "registry port")
        );
        let mut i = input();
        i.registry = true;
        let c = prereq_checks(&h, &l, &i);
        assert_eq!(by_name(&c, "registry port").status, Status::Warn);
    }

    #[test]
    fn daemon_checks_are_warnings_not_failures() {
        let mut h = FakeHost::healthy_host();
        let input = DaemonInput {
            cloudflared_unit: Some("cloudflared".into()),
            traefik_unit: Some("traefik".into()),
            frps_unit: None,
        };
        let c = daemon_checks(&h, &input);
        assert_eq!(c.len(), 2);
        assert!(c.iter().all(|c| c.status == Status::Warn));

        h.binaries.insert("cloudflared".into());
        h.respond("systemctl cat cloudflared.service", CmdOutput::ok("[Unit]"));
        h.respond(
            "systemctl is-enabled --quiet cloudflared.service",
            CmdOutput::ok(""),
        );
        h.respond(
            "systemctl is-active --quiet cloudflared.service",
            CmdOutput::ok(""),
        );
        let c = daemon_checks(&h, &input);
        assert_eq!(by_name(&c, "daemon cloudflared").status, Status::Ok);
        assert_eq!(by_name(&c, "daemon traefik").status, Status::Warn);
    }

    #[test]
    fn table_prints_hints_for_non_ok_only() {
        let t = render_table(&[
            Check::new("a", Status::Ok, "fine").hint("hidden"),
            Check::new("b", Status::Fail, "bad").hint("do this"),
        ]);
        assert!(t.contains("hint: do this"));
        assert!(!t.contains("hidden"));
    }
}
