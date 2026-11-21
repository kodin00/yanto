//! `frps.toml` model (docs/rewrite/04 8.4): read the fields Yanto owns, change only those, and
//! keep every other key, table and comment byte-for-byte (comment-preserving `toml_edit`).
//! `auth.token` and `webServer.password` are secrets: they appear only as [`Secret`] and are masked
//! in every diff and raw view.

use std::net::IpAddr;

use serde::Serialize;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::error::{CoreError, FieldError};
use crate::secrets::Secret;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/frp/frps.toml";
pub const MASK: &str = "********";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PortRange {
    Single { port: u16 },
    Range { start: u16, end: u16 },
}

impl PortRange {
    fn contains(&self, p: u16) -> bool {
        match self {
            PortRange::Single { port } => *port == p,
            PortRange::Range { start, end } => (*start..=*end).contains(&p),
        }
    }
}

/// The owned fields as currently in the file.
#[derive(Debug, Clone, Default)]
pub struct Owned {
    pub bind_addr: Option<String>,
    pub bind_port: Option<u16>,
    pub auth_method: Option<String>,
    pub auth_token: Option<Secret>,
    pub web_addr: Option<String>,
    pub web_port: Option<u16>,
    pub web_user: Option<String>,
    pub web_password: Option<Secret>,
    pub allow_ports: Vec<PortRange>,
}

/// Change of one owned field.
#[derive(Debug, Clone, Default)]
pub enum Change<T> {
    #[default]
    Keep,
    Set(T),
    Clear,
}

#[derive(Debug, Clone, Default)]
pub struct Patch {
    pub bind_addr: Change<String>,
    pub bind_port: Change<u16>,
    pub auth_method: Change<String>,
    pub auth_token: Change<Secret>,
    pub web_addr: Change<String>,
    pub web_port: Change<u16>,
    pub web_user: Change<String>,
    pub web_password: Change<Secret>,
    pub allow_ports: Change<Vec<PortRange>>,
}

fn invalid(e: toml_edit::TomlError) -> CoreError {
    CoreError::validation("config", format!("frps.toml is not valid TOML: {e}"))
}

pub fn parse(text: &str) -> Result<DocumentMut, CoreError> {
    text.parse::<DocumentMut>().map_err(invalid)
}

fn get_str(doc: &DocumentMut, path: &[&str]) -> Option<String> {
    let mut cur: &Item = doc.as_item();
    for k in path {
        cur = cur.as_table_like()?.get(k)?;
    }
    cur.as_str().map(str::to_owned)
}

fn get_port(doc: &DocumentMut, path: &[&str]) -> Option<u16> {
    let mut cur: &Item = doc.as_item();
    for k in path {
        cur = cur.as_table_like()?.get(k)?;
    }
    cur.as_integer().and_then(|n| u16::try_from(n).ok())
}

pub fn read_owned(doc: &DocumentMut) -> Owned {
    let mut allow = Vec::new();
    if let Some(arr) = doc.get("allowPorts").and_then(Item::as_array) {
        for v in arr {
            let Some(t) = v.as_inline_table() else {
                continue;
            };
            let n = |k: &str| {
                t.get(k)
                    .and_then(Value::as_integer)
                    .and_then(|n| u16::try_from(n).ok())
            };
            if let Some(port) = n("single") {
                allow.push(PortRange::Single { port });
            } else if let (Some(start), Some(end)) = (n("start"), n("end")) {
                allow.push(PortRange::Range { start, end });
            }
        }
    }
    Owned {
        bind_addr: get_str(doc, &["bindAddr"]),
        bind_port: get_port(doc, &["bindPort"]),
        auth_method: get_str(doc, &["auth", "method"]),
        auth_token: get_str(doc, &["auth", "token"]).map(Secret::new),
        web_addr: get_str(doc, &["webServer", "addr"]),
        web_port: get_port(doc, &["webServer", "port"]),
        web_user: get_str(doc, &["webServer", "user"]),
        web_password: get_str(doc, &["webServer", "password"]).map(Secret::new),
        allow_ports: allow,
    }
}

fn is_loopback(addr: &str) -> bool {
    addr.parse::<IpAddr>().is_ok_and(|a| a.is_loopback()) || addr == "localhost"
}

impl Owned {
    /// The dashboard address frps listens on (`127.0.0.1` when `webServer.addr` is unset).
    pub fn dashboard_addr(&self) -> String {
        self.web_addr.clone().unwrap_or_else(|| "127.0.0.1".into())
    }

    /// `true` when the dashboard is enabled on a non-loopback address.
    pub fn dashboard_exposed(&self) -> bool {
        self.web_port.is_some() && !is_loopback(&self.dashboard_addr())
    }

    /// Validation shared by save: ports in range, ordered ranges, no duplicate singles, no overlap
    /// with `bindPort`/`webServer.port`, dashboard credentials when enabled.
    pub fn validate(&self) -> Result<(), CoreError> {
        let mut errs: Vec<FieldError> = Vec::new();
        let mut push = |f: &str, m: &str| {
            errs.push(FieldError {
                field: f.to_owned(),
                message: m.to_owned(),
            })
        };
        for (f, a) in [("bind_addr", &self.bind_addr), ("web_addr", &self.web_addr)] {
            if let Some(a) = a
                && a.parse::<IpAddr>().is_err()
            {
                push(f, "must be an IP address such as 127.0.0.1 or 0.0.0.0");
            }
        }
        for (f, p) in [("bind_port", self.bind_port), ("web_port", self.web_port)] {
            if p == Some(0) {
                push(f, "port must be 1-65535");
            }
        }
        if self.bind_port.is_some() && self.bind_port == self.web_port {
            push("web_port", "dashboard port must differ from bind port");
        }
        let mut singles = std::collections::BTreeSet::new();
        for r in &self.allow_ports {
            match r {
                PortRange::Single { port } => {
                    if *port == 0 {
                        push("allow_ports", "ports must be 1-65535");
                    }
                    if !singles.insert(*port) {
                        push("allow_ports", &format!("duplicate single port {port}"));
                    }
                }
                PortRange::Range { start, end } => {
                    if *start == 0 || start > end {
                        push(
                            "allow_ports",
                            &format!("range {start}-{end} is not ordered"),
                        );
                    }
                }
            }
            for (f, p) in [("bind_port", self.bind_port), ("web_port", self.web_port)] {
                if let Some(p) = p
                    && r.contains(p)
                {
                    push("allow_ports", &format!("allowPorts overlaps {f} {p}"));
                }
            }
        }
        if let Some(m) = &self.auth_method
            && m != "token"
            && m != "oidc"
        {
            push("auth_method", "must be token or oidc");
        }
        if self.web_port.is_some() {
            let user = self.web_user.as_deref().unwrap_or("");
            let pw = self.web_password.as_ref().map_or("", |p| p.expose());
            if user.is_empty() || pw.is_empty() {
                push(
                    "web_user",
                    "the dashboard needs a user and a password when enabled",
                );
            }
        }
        if errs.is_empty() {
            Ok(())
        } else {
            let msg = errs
                .iter()
                .map(|e| format!("{}: {}", e.field, e.message))
                .collect::<Vec<_>>()
                .join("; ");
            Err(CoreError::new(crate::ErrorKind::Validation(errs), msg))
        }
    }
}

/// Resolve a patch against the current owned fields (what the file would hold after saving).
pub fn resolve(current: &Owned, p: &Patch) -> Owned {
    fn pick<T: Clone>(cur: &Option<T>, c: &Change<T>) -> Option<T> {
        match c {
            Change::Keep => cur.clone(),
            Change::Set(v) => Some(v.clone()),
            Change::Clear => None,
        }
    }
    Owned {
        bind_addr: pick(&current.bind_addr, &p.bind_addr),
        bind_port: pick(&current.bind_port, &p.bind_port),
        auth_method: pick(&current.auth_method, &p.auth_method),
        auth_token: pick(&current.auth_token, &p.auth_token),
        web_addr: pick(&current.web_addr, &p.web_addr),
        web_port: pick(&current.web_port, &p.web_port),
        web_user: pick(&current.web_user, &p.web_user),
        web_password: pick(&current.web_password, &p.web_password),
        allow_ports: match &p.allow_ports {
            Change::Keep => current.allow_ports.clone(),
            Change::Set(v) => v.clone(),
            Change::Clear => Vec::new(),
        },
    }
}

fn set_scalar(parent: &mut dyn toml_edit::TableLike, key: &str, v: Value) {
    match parent.get_mut(key) {
        // Keep the user's surrounding whitespace and trailing comment.
        Some(Item::Value(old)) => {
            let decor = old.decor().clone();
            let mut nv = v;
            *nv.decor_mut() = decor;
            *old = nv;
        }
        _ => {
            parent.insert(key, value(v));
        }
    }
}

fn nested<'a>(doc: &'a mut DocumentMut, table: &str) -> &'a mut dyn toml_edit::TableLike {
    if doc.get(table).and_then(Item::as_table_like).is_none() {
        doc.insert(table, Item::Table(Table::new()));
    }
    doc.get_mut(table)
        .and_then(Item::as_table_like_mut)
        .expect("table just ensured")
}

fn apply_nested<T>(
    doc: &mut DocumentMut,
    table: &str,
    key: &str,
    c: &Change<T>,
    to_value: impl Fn(&T) -> Value,
) {
    match c {
        Change::Keep => {}
        Change::Set(v) => set_scalar(nested(doc, table), key, to_value(v)),
        Change::Clear => {
            if let Some(t) = doc.get_mut(table).and_then(Item::as_table_like_mut) {
                t.remove(key);
            }
        }
    }
}

/// Apply a patch to the document, touching only the owned keys.
pub fn apply(doc: &mut DocumentMut, p: &Patch) {
    macro_rules! root {
        ($c:expr, $key:expr, $conv:expr) => {
            match &$c {
                Change::Keep => {}
                Change::Set(v) => set_scalar(doc.as_table_mut(), $key, $conv(v)),
                Change::Clear => {
                    doc.remove($key);
                }
            }
        };
    }
    root!(p.bind_addr, "bindAddr", |v: &String| Value::from(
        v.as_str()
    ));
    root!(p.bind_port, "bindPort", |v: &u16| Value::from(i64::from(
        *v
    )));
    apply_nested(doc, "auth", "method", &p.auth_method, |v| {
        Value::from(v.as_str())
    });
    apply_nested(doc, "auth", "token", &p.auth_token, |v| {
        Value::from(v.expose())
    });
    apply_nested(doc, "webServer", "addr", &p.web_addr, |v| {
        Value::from(v.as_str())
    });
    apply_nested(doc, "webServer", "port", &p.web_port, |v| {
        Value::from(i64::from(*v))
    });
    apply_nested(doc, "webServer", "user", &p.web_user, |v| {
        Value::from(v.as_str())
    });
    apply_nested(doc, "webServer", "password", &p.web_password, |v| {
        Value::from(v.expose())
    });
    match &p.allow_ports {
        Change::Keep => {}
        Change::Clear => {
            doc.remove("allowPorts");
        }
        Change::Set(list) => {
            let mut arr = Array::new();
            for r in list {
                let mut t = InlineTable::new();
                match r {
                    PortRange::Single { port } => {
                        t.insert("single", i64::from(*port).into());
                    }
                    PortRange::Range { start, end } => {
                        t.insert("start", i64::from(*start).into());
                        t.insert("end", i64::from(*end).into());
                    }
                }
                arr.push(Value::InlineTable(t));
            }
            set_scalar(doc.as_table_mut(), "allowPorts", Value::Array(arr));
        }
    }
    // Drop tables the patch emptied (an empty `[auth]` is noise).
    for t in ["auth", "webServer"] {
        if doc
            .get(t)
            .and_then(Item::as_table_like)
            .is_some_and(|x| x.is_empty())
        {
            doc.remove(t);
        }
    }
}

// ---- masking ------------------------------------------------------------------------------

fn secret_key(line: &str) -> Option<usize> {
    let (k, _) = line.split_once('=')?;
    let k = k.trim().to_ascii_lowercase();
    if k.starts_with('#') {
        return None;
    }
    ["token", "password", "secret"]
        .iter()
        .any(|w| k.contains(w))
        .then(|| line.find('=').expect("has ="))
}

/// Replace the value of every `*token*`/`*password*`/`*secret*` line with a mask. Line based, so
/// it also masks secrets in keys Yanto does not own.
pub fn mask_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match secret_key(line) {
            Some(eq) => {
                let nl = if line.ends_with('\n') { "\n" } else { "" };
                out.push_str(&format!("{} = \"{MASK}\"{nl}", line[..eq].trim_end()));
            }
            None => out.push_str(line),
        }
    }
    out
}

/// Everything Yanto does not own, as masked TOML (read-only view).
pub fn unowned_view(doc: &DocumentMut) -> String {
    let mut d = doc.clone();
    for k in ["bindAddr", "bindPort", "allowPorts"] {
        d.remove(k);
    }
    for (t, keys) in [
        ("auth", &["method", "token"][..]),
        ("webServer", &["addr", "port", "user", "password"][..]),
    ] {
        if let Some(tl) = d.get_mut(t).and_then(Item::as_table_like_mut) {
            for k in keys {
                tl.remove(k);
            }
        }
        if d.get(t)
            .and_then(Item::as_table_like)
            .is_some_and(|x| x.is_empty())
        {
            d.remove(t);
        }
    }
    mask_secrets(d.to_string().trim_start_matches('\n'))
}

/// Minimal masked line diff for the confirmation view: common prefix and suffix are folded.
pub fn masked_diff(old: &str, new: &str) -> Vec<String> {
    let (o, n): (Vec<String>, Vec<String>) = (
        mask_secrets(old).lines().map(str::to_owned).collect(),
        mask_secrets(new).lines().map(str::to_owned).collect(),
    );
    let mut pre = 0;
    while pre < o.len() && pre < n.len() && o[pre] == n[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < o.len() - pre && suf < n.len() - pre && o[o.len() - 1 - suf] == n[n.len() - 1 - suf]
    {
        suf += 1;
    }
    let mut out: Vec<String> = o[pre..o.len() - suf]
        .iter()
        .map(|l| format!("- {l}"))
        .collect();
    out.extend(n[pre..n.len() - suf].iter().map(|l| format!("+ {l}")));
    out
}

/// `true` when the file mode lets other users read it (it can hold secrets).
pub fn world_readable(mode: u32) -> bool {
    mode & 0o004 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# frps config, hand edited
bindPort = 7000   # control port
vhostHTTPPort = 8080

# dashboard
[webServer]
addr = \"127.0.0.1\"
port = 7500
user = \"admin\"
password = \"hunter2\"

[transport]
maxPoolCount = 5 # keep
";

    fn patch_port(n: u16) -> Patch {
        Patch {
            bind_port: Change::Set(n),
            ..Default::default()
        }
    }

    #[test]
    fn reads_owned_fields() {
        let o = read_owned(&parse(SAMPLE).unwrap());
        assert_eq!(o.bind_port, Some(7000));
        assert_eq!(o.web_port, Some(7500));
        assert_eq!(o.web_user.as_deref(), Some("admin"));
        assert_eq!(o.web_password.as_ref().unwrap().expose(), "hunter2");
        assert!(!o.dashboard_exposed());
    }

    #[test]
    fn editing_preserves_everything_else_byte_for_byte() {
        let mut doc = parse(SAMPLE).unwrap();
        apply(&mut doc, &patch_port(7001));
        let got = doc.to_string();
        assert_eq!(
            got,
            SAMPLE.replace("bindPort = 7000   #", "bindPort = 7001   #")
        );
    }

    #[test]
    fn adds_missing_keys_and_the_result_reparses() {
        let mut doc = parse(SAMPLE).unwrap();
        let p = Patch {
            bind_addr: Change::Set("0.0.0.0".into()),
            auth_method: Change::Set("token".into()),
            auth_token: Change::Set(Secret::new("tok")),
            allow_ports: Change::Set(vec![
                PortRange::Range {
                    start: 2000,
                    end: 3000,
                },
                PortRange::Single { port: 3001 },
            ]),
            ..Default::default()
        };
        apply(&mut doc, &p);
        let text = doc.to_string();
        assert!(text.contains("vhostHTTPPort = 8080"));
        assert!(text.contains("maxPoolCount = 5 # keep"));
        let o = read_owned(&parse(&text).unwrap());
        assert_eq!(o.bind_addr.as_deref(), Some("0.0.0.0"));
        assert_eq!(o.auth_token.as_ref().unwrap().expose(), "tok");
        assert_eq!(o.allow_ports.len(), 2);
        assert_eq!(o.bind_port, Some(7000));
        // untouched keys outside the owned set are unchanged
        let t: toml_edit::DocumentMut = text.parse().unwrap();
        assert_eq!(t["transport"]["maxPoolCount"].as_integer(), Some(5));
    }

    #[test]
    fn clearing_removes_keys_and_empty_tables() {
        let mut doc = parse(SAMPLE).unwrap();
        let p = Patch {
            web_addr: Change::Clear,
            web_port: Change::Clear,
            web_user: Change::Clear,
            web_password: Change::Clear,
            ..Default::default()
        };
        apply(&mut doc, &p);
        assert!(!doc.to_string().contains("webServer"));
        assert!(doc.to_string().contains("[transport]"));
    }

    #[test]
    fn dotted_keys_are_edited_in_place() {
        let mut doc =
            parse("auth.method = \"token\"\nauth.token = \"old\"\nbindPort = 7000\n").unwrap();
        apply(
            &mut doc,
            &Patch {
                auth_token: Change::Set(Secret::new("new")),
                ..Default::default()
            },
        );
        assert_eq!(
            doc.to_string(),
            "auth.method = \"token\"\nauth.token = \"new\"\nbindPort = 7000\n"
        );
    }

    #[test]
    fn invalid_toml_is_rejected() {
        assert!(parse("bindPort = = 1").is_err());
    }

    fn owned(f: impl FnOnce(&mut Owned)) -> Owned {
        let mut o = Owned {
            bind_port: Some(7000),
            ..Default::default()
        };
        f(&mut o);
        o
    }

    #[test]
    fn validation_rules() {
        assert!(owned(|_| {}).validate().is_ok());
        assert!(owned(|o| o.bind_port = Some(0)).validate().is_err());
        assert!(
            owned(|o| o.bind_addr = Some("not an ip".into()))
                .validate()
                .is_err()
        );
        assert!(
            owned(|o| o.allow_ports = vec![PortRange::Range { start: 9, end: 3 }])
                .validate()
                .is_err()
        );
        assert!(
            owned(|o| o.allow_ports =
                vec![PortRange::Single { port: 5 }, PortRange::Single { port: 5 }])
            .validate()
            .is_err()
        );
        assert!(
            owned(|o| o.allow_ports = vec![PortRange::Range {
                start: 6000,
                end: 8000
            }])
            .validate()
            .is_err(),
            "range contains bindPort"
        );
        assert!(owned(|o| o.web_port = Some(7000)).validate().is_err());
        assert!(
            owned(|o| o.web_port = Some(7500)).validate().is_err(),
            "dashboard needs credentials"
        );
        assert!(
            owned(|o| {
                o.web_port = Some(7500);
                o.web_user = Some("u".into());
                o.web_password = Some(Secret::new("p"));
            })
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn exposure_flag() {
        let o = owned(|o| {
            o.web_port = Some(7500);
            o.web_addr = Some("0.0.0.0".into());
        });
        assert!(o.dashboard_exposed());
    }

    #[test]
    fn secrets_are_masked_in_views_and_diffs() {
        let mut doc = parse(SAMPLE).unwrap();
        apply(
            &mut doc,
            &Patch {
                web_password: Change::Set(Secret::new("brand-new-pw")),
                bind_port: Change::Set(7002),
                ..Default::default()
            },
        );
        let diff = masked_diff(SAMPLE, &doc.to_string()).join("\n");
        assert!(
            !diff.contains("hunter2") && !diff.contains("brand-new-pw"),
            "{diff}"
        );
        assert!(diff.contains("bindPort"));
        let raw = unowned_view(
            &parse("auth.token = \"x\"\n[transport]\ntlsKeyPassword = \"zzz\"\nmaxPoolCount = 5\n")
                .unwrap(),
        );
        assert!(!raw.contains("zzz") && !raw.contains("\"x\""), "{raw}");
        assert!(raw.contains("maxPoolCount"));
        assert!(world_readable(0o644) && !world_readable(0o600));
    }
}
