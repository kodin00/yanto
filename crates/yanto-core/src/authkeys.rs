//! Authorized Keys: pure domain logic (docs/rewrite/04 section 6).
//!
//! * [`parse_public_key`]: strict validation of one OpenSSH public key line (6.3), using the
//!   `ssh-key` crate, and the canonical line Yanto writes.
//! * [`splice`]: marked-block surgery on the raw bytes of `authorized_keys` (6.4). Everything
//!   outside the block is preserved byte for byte, including CRLF lines and a missing final newline.
//! * [`count_valid_keys`]: the lockout guard's counter (6.5, Rule 1).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ssh_key::public::KeyData;
use ssh_key::{HashAlg, PublicKey};

use crate::error::CoreError;

pub const BEGIN_MARKER: &str = "# BEGIN YANTO MANAGED KEYS (do not edit between these lines)";
pub const END_MARKER: &str = "# END YANTO MANAGED KEYS";
const BEGIN_PREFIX: &str = "# BEGIN YANTO MANAGED";
const END_PREFIX: &str = "# END YANTO MANAGED";
pub const MAX_KEY_INPUT_BYTES: usize = 8 * 1024;
pub const MAX_NAME_CHARS: usize = 64;

/// Key types Yanto accepts (6.3). `ssh-dss` is deliberately absent.
const ALLOWED_TYPES: [&str; 7] = [
    "ssh-ed25519",
    "sk-ssh-ed25519@openssh.com",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ecdsa-sha2-nistp256@openssh.com",
    "ssh-rsa",
];

/// A key that passed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedKey {
    pub key_type: String,
    /// `type base64` without a comment; the form stored in `authorized_keys.public_key`.
    pub public_key: String,
    /// `SHA256:<base64>` as printed by `ssh-keygen -lf`.
    pub fingerprint: String,
    /// The comment the operator pasted, if any (only ever used as a name suggestion).
    pub comment: String,
    /// Non-blocking remark, for example a short RSA key.
    pub warning: Option<String>,
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::validation("public_key", message)
}

/// Validate one OpenSSH public key line. Options, multiple lines, private keys, DSA and short RSA are rejected.
pub fn parse_public_key(input: &str) -> Result<ParsedKey, CoreError> {
    if input.len() > MAX_KEY_INPUT_BYTES {
        return Err(invalid("key is longer than 8 KiB"));
    }
    let text = input.trim();
    if text.is_empty() {
        return Err(invalid("paste one public key line"));
    }
    if text.contains("-----BEGIN") || text.contains("PRIVATE KEY") {
        return Err(invalid(
            "this looks like private key material; paste the public key (.pub) instead",
        ));
    }
    if text.contains(['\n', '\r']) {
        return Err(invalid("paste exactly one key on a single line"));
    }
    if text.chars().any(char::is_control) {
        return Err(invalid("key contains control characters"));
    }
    let mut fields = text.split_whitespace();
    let key_type = fields.next().unwrap_or_default();
    let blob = fields.next();
    let comment = fields.collect::<Vec<_>>().join(" ");
    if key_type == "ssh-dss" {
        return Err(invalid("ssh-dss (DSA) keys are not supported"));
    }
    if !ALLOWED_TYPES.contains(&key_type) {
        let looks_like_options = key_type.contains('=') || key_type.contains(',');
        return Err(invalid(if looks_like_options {
            "key options (command=, from=, ...) are not supported; paste only `type key [comment]`"
        } else {
            "unsupported key type (use ssh-ed25519, ecdsa-sha2-*, sk-*, or RSA of at least 2048 bits)"
        }));
    }
    let blob = blob.ok_or_else(|| invalid("the key data is missing after the key type"))?;
    let key = PublicKey::from_openssh(&format!("{key_type} {blob}"))
        .map_err(|_| invalid("not a valid public key (bad base64 or type mismatch)"))?;
    if key.algorithm().as_str() != key_type {
        return Err(invalid("the key type does not match the key data"));
    }
    let mut warning = None;
    if let KeyData::Rsa(rsa) = key.key_data() {
        let bits = rsa
            .n
            .as_positive_bytes()
            .map_or(0, |b| b.len().saturating_mul(8));
        if bits < 2048 {
            return Err(invalid(format!(
                "RSA keys must be at least 2048 bits (this one is {bits})"
            )));
        }
        if bits < 3072 {
            warning = Some(format!(
                "RSA key of {bits} bits is accepted, but 3072 bits or more is recommended"
            ));
        }
    }
    Ok(ParsedKey {
        key_type: key_type.to_owned(),
        public_key: format!("{key_type} {blob}"),
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        comment,
        warning,
    })
}

/// Validate a display name, or derive one from the pasted comment when `name` is blank.
/// Allowed: `[A-Za-z0-9._@ -]{1,64}`.
pub fn resolve_name(name: &str, comment: &str) -> Result<String, CoreError> {
    let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '@' | ' ' | '-');
    let name = name.trim();
    if !name.is_empty() {
        if name.chars().count() > MAX_NAME_CHARS || !name.chars().all(ok) {
            return Err(CoreError::validation(
                "name",
                "name must be 1-64 characters from letters, digits, . _ @ - and space",
            ));
        }
        return Ok(name.to_owned());
    }
    let derived: String = comment
        .chars()
        .map(|c| if ok(c) { c } else { '_' })
        .take(MAX_NAME_CHARS)
        .collect();
    let derived = derived.trim().to_owned();
    Ok(if derived.is_empty() {
        "key".into()
    } else {
        derived
    })
}

/// The canonical line written into the block: `type base64 yanto:<id>:<name>` (6.3).
pub fn managed_line(id: &str, name: &str, public_key: &str) -> String {
    format!("{public_key} yanto:{id}:{name}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpliceError {
    /// BEGIN without END, END before BEGIN, two blocks, or an inexact marker (6.4 step 4).
    AmbiguousMarkers(String),
}

impl std::fmt::Display for SpliceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpliceError::AmbiguousMarkers(m) => write!(f, "ambiguous markers ({m}); fix manually"),
        }
    }
}

/// Result of [`splice`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spliced {
    pub bytes: Vec<u8>,
    /// What [`outside_block`] of `bytes` must equal (verification, 6.4 step 7).
    pub expected_outside: Vec<u8>,
}

/// Lines of `data` keeping their terminators.
fn lines(data: &[u8]) -> Vec<&[u8]> {
    data.split_inclusive(|b| *b == b'\n').collect()
}

fn content(line: &[u8]) -> &[u8] {
    let l = line.strip_suffix(b"\n").unwrap_or(line);
    l.strip_suffix(b"\r").unwrap_or(l)
}

#[derive(Debug, PartialEq, Eq)]
enum Block {
    Absent,
    /// Indices of the BEGIN and END lines.
    One(usize, usize),
}

fn find_block(ls: &[&[u8]]) -> Result<Block, SpliceError> {
    let (mut begins, mut ends) = (Vec::new(), Vec::new());
    for (i, l) in ls.iter().enumerate() {
        let c = content(l);
        if c == BEGIN_MARKER.as_bytes() {
            begins.push(i);
        } else if c == END_MARKER.as_bytes() {
            ends.push(i);
        } else if c.starts_with(BEGIN_PREFIX.as_bytes()) || c.starts_with(END_PREFIX.as_bytes()) {
            return Err(SpliceError::AmbiguousMarkers(
                "a Yanto marker line was edited".into(),
            ));
        }
    }
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => Ok(Block::Absent),
        ([b], [e]) if b < e => Ok(Block::One(*b, *e)),
        ([_], [_]) => Err(SpliceError::AmbiguousMarkers(
            "END comes before BEGIN".into(),
        )),
        ([_], []) => Err(SpliceError::AmbiguousMarkers("BEGIN without END".into())),
        ([], [_]) => Err(SpliceError::AmbiguousMarkers("END without BEGIN".into())),
        _ => Err(SpliceError::AmbiguousMarkers("more than one block".into())),
    }
}

/// Everything except the marker-delimited block (markers included). Errors on ambiguous markers.
pub fn outside_block(existing: &[u8]) -> Result<Vec<u8>, SpliceError> {
    let ls = lines(existing);
    Ok(match find_block(&ls)? {
        Block::Absent => existing.to_vec(),
        Block::One(b, e) => ls[..b]
            .iter()
            .chain(ls[e + 1..].iter())
            .flat_map(|l| l.iter().copied())
            .collect(),
    })
}

/// Lines currently inside the block, without terminators (empty when there is no block).
pub fn block_lines(existing: &[u8]) -> Result<Vec<String>, SpliceError> {
    let ls = lines(existing);
    Ok(match find_block(&ls)? {
        Block::Absent => Vec::new(),
        Block::One(b, e) => ls[b + 1..e]
            .iter()
            .map(|l| String::from_utf8_lossy(content(l)).into_owned())
            .collect(),
    })
}

/// Put `block` between the markers. No block yet: append one (adding a newline first when the
/// existing content lacks one), or leave the file untouched when `block` is empty. A block that
/// exists is rewritten in place; lines outside are copied byte for byte.
pub fn splice(existing: &[u8], block: &[String]) -> Result<Spliced, SpliceError> {
    let ls = lines(existing);
    let state = find_block(&ls)?;
    let mut region = Vec::new();
    region.extend_from_slice(BEGIN_MARKER.as_bytes());
    region.push(b'\n');
    for l in block {
        region.extend_from_slice(l.as_bytes());
        region.push(b'\n');
    }
    region.extend_from_slice(END_MARKER.as_bytes());
    region.push(b'\n');
    match state {
        Block::Absent if block.is_empty() => Ok(Spliced {
            bytes: existing.to_vec(),
            expected_outside: existing.to_vec(),
        }),
        Block::Absent => {
            let mut bytes = existing.to_vec();
            if !bytes.is_empty() && !bytes.ends_with(b"\n") {
                bytes.push(b'\n');
            }
            let expected_outside = bytes.clone();
            bytes.extend_from_slice(&region);
            Ok(Spliced {
                bytes,
                expected_outside,
            })
        }
        Block::One(b, e) => {
            let mut bytes = Vec::with_capacity(existing.len() + region.len());
            for l in &ls[..b] {
                bytes.extend_from_slice(l);
            }
            bytes.extend_from_slice(&region);
            for l in &ls[e + 1..] {
                bytes.extend_from_slice(l);
            }
            let expected_outside = ls[..b]
                .iter()
                .chain(ls[e + 1..].iter())
                .flat_map(|l| l.iter().copied())
                .collect();
            Ok(Spliced {
                bytes,
                expected_outside,
            })
        }
    }
}

/// Whether one `authorized_keys` line holds a key: optional options, then a key type followed by
/// base64 whose embedded type string equals it (so certificates count too).
pub fn line_has_key(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return false;
    }
    let fields = split_fields(line);
    fields.windows(2).any(|w| {
        let t = w[0];
        (t.starts_with("ssh-") || t.starts_with("ecdsa-") || t.starts_with("sk-"))
            && STANDARD.decode(w[1]).is_ok_and(|b| {
                b.len() > 4 + t.len()
                    && b[..4] == u32::try_from(t.len()).unwrap_or(u32::MAX).to_be_bytes()
                    && &b[4..4 + t.len()] == t.as_bytes()
            })
    })
}

/// Whitespace-separated fields; whitespace inside double quotes (options) does not split.
fn split_fields(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut in_quotes, mut escaped) = (None, false, false);
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_quotes => escaped = true,
            '"' => {
                in_quotes = !in_quotes;
                start.get_or_insert(i);
            }
            c if c.is_whitespace() && !in_quotes => {
                if let Some(s) = start.take() {
                    out.push(&line[s..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push(&line[s..]);
    }
    out
}

/// Number of key lines in a whole `authorized_keys` file (inside and outside the block).
pub fn count_valid_keys(data: &[u8]) -> usize {
    lines(data)
        .into_iter()
        .filter(|l| line_has_key(&String::from_utf8_lossy(content(l))))
        .count()
}

/// Fingerprints of the keys found outside the block (best effort; unparsable lines are skipped).
pub fn outside_fingerprints(outside: &[u8]) -> Vec<String> {
    lines(outside)
        .into_iter()
        .filter_map(|l| {
            let line = String::from_utf8_lossy(content(l)).into_owned();
            let f = split_fields(&line);
            f.windows(2)
                .find_map(|w| parse_public_key(&format!("{} {}", w[0], w[1])).ok())
                .map(|k| k.fingerprint)
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod test_keys {
    use super::*;

    fn string(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_be_bytes());
        out.extend_from_slice(bytes);
    }

    /// A syntactically valid ed25519 public key made from a seed byte (not a real key pair).
    pub fn ed25519(seed: u8, comment: &str) -> String {
        let mut blob = Vec::new();
        string(&mut blob, b"ssh-ed25519");
        string(&mut blob, &[seed; 32]);
        format!("ssh-ed25519 {} {comment}", STANDARD.encode(blob))
            .trim()
            .to_owned()
    }

    /// A syntactically valid RSA public key with a modulus of `bits` bits.
    pub fn rsa(bits: usize, comment: &str) -> String {
        let mut blob = Vec::new();
        string(&mut blob, b"ssh-rsa");
        string(&mut blob, &[0x01, 0x00, 0x01]);
        let mut n = vec![0xC5; bits / 8];
        n.insert(0, 0);
        string(&mut blob, &n);
        format!("ssh-rsa {} {comment}", STANDARD.encode(blob))
    }
}

#[cfg(test)]
mod tests {
    use super::test_keys::*;
    use super::*;

    fn block(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn accepts_ed25519_and_computes_fingerprint() {
        let k = parse_public_key(&ed25519(7, "laptop")).unwrap();
        assert_eq!(k.key_type, "ssh-ed25519");
        assert!(k.fingerprint.starts_with("SHA256:"));
        assert!(!k.public_key.contains("laptop"));
        assert_eq!(k.comment, "laptop");
        assert!(k.warning.is_none());
        // Same bytes, different comment: same fingerprint.
        assert_eq!(
            parse_public_key(&ed25519(7, "other")).unwrap().fingerprint,
            k.fingerprint
        );
        assert_ne!(
            parse_public_key(&ed25519(8, "x")).unwrap().fingerprint,
            k.fingerprint
        );
    }

    #[test]
    fn rejects_options_multiline_private_dsa_and_garbage() {
        let good = ed25519(1, "a");
        let cases = [
            (format!("command=\"rm -rf /\" {good}"), "options"),
            (format!("{good}\n{good}"), "single line"),
            ("-----BEGIN OPENSSH PRIVATE KEY-----".to_owned(), "private"),
            ("ssh-dss AAAAB3NzaC1kc3M=".to_owned(), "DSA"),
            (String::new(), "paste"),
            ("ssh-ed25519".to_owned(), "missing"),
            ("ssh-ed25519 !!!notbase64".to_owned(), "valid public key"),
            ("ssh-rsa AAAA".to_owned(), "valid public key"),
            (
                format!("{}{}", good, "a".repeat(MAX_KEY_INPUT_BYTES)),
                "8 KiB",
            ),
            (format!("ssh-ed25519\u{7} {good}"), "control"),
        ];
        for (input, needle) in cases {
            let e = parse_public_key(&input).unwrap_err();
            assert!(
                e.message.contains(needle),
                "{input:?}: expected {needle:?} in {:?}",
                e.message
            );
        }
        // Declared type must equal the embedded type.
        let wrong = good.replacen("ssh-ed25519", "ecdsa-sha2-nistp256", 1);
        assert!(parse_public_key(&wrong).is_err());
    }

    #[test]
    fn rsa_size_policy() {
        assert!(parse_public_key(&rsa(1024, "")).is_err());
        let mid = parse_public_key(&rsa(2048, "")).unwrap();
        assert!(mid.warning.is_some());
        let big = parse_public_key(&rsa(4096, "")).unwrap();
        assert!(big.warning.is_none());
    }

    #[test]
    fn names() {
        assert_eq!(resolve_name("My Laptop-1", "").unwrap(), "My Laptop-1");
        assert!(resolve_name("bad;name", "").is_err());
        assert!(resolve_name(&"a".repeat(65), "").is_err());
        assert_eq!(resolve_name("", "me@host:~/x").unwrap(), "me@host___x");
        assert_eq!(resolve_name("", "").unwrap(), "key");
        let line = managed_line("akey_1", "n", "ssh-ed25519 AAAA");
        assert_eq!(line, "ssh-ed25519 AAAA yanto:akey_1:n");
    }

    #[test]
    fn append_when_absent_keeps_outside_bytes() {
        let existing = b"ssh-ed25519 AAAA manual\r\n# note\r\nlast-line-without-newline";
        let out = splice(existing, &block(&["k1"])).unwrap();
        let text = String::from_utf8(out.bytes.clone()).unwrap();
        assert!(
            text.starts_with("ssh-ed25519 AAAA manual\r\n# note\r\nlast-line-without-newline\n")
        );
        assert!(text.ends_with(&format!("{BEGIN_MARKER}\nk1\n{END_MARKER}\n")));
        assert_eq!(outside_block(&out.bytes).unwrap(), out.expected_outside);
        // The original bytes are an exact prefix apart from the one added newline.
        assert_eq!(&out.expected_outside[..existing.len()], existing);
    }

    #[test]
    fn replace_existing_block_leaves_everything_else_identical() {
        let before = b"first\r\n# x\n".to_vec();
        let after = b"tail without newline".to_vec();
        let mut file = before.clone();
        file.extend_from_slice(format!("{BEGIN_MARKER}\nold1\nold2\n{END_MARKER}\n").as_bytes());
        file.extend_from_slice(&after);
        let out = splice(&file, &block(&["new"])).unwrap();
        let mut expected = before.clone();
        expected.extend_from_slice(format!("{BEGIN_MARKER}\nnew\n{END_MARKER}\n").as_bytes());
        expected.extend_from_slice(&after);
        assert_eq!(out.bytes, expected);
        let mut outside = before;
        outside.extend_from_slice(&after);
        assert_eq!(out.expected_outside, outside);
        assert_eq!(outside_block(&out.bytes).unwrap(), outside);
        assert_eq!(block_lines(&out.bytes).unwrap(), vec!["new"]);
        // Idempotent.
        assert_eq!(
            splice(&out.bytes, &block(&["new"])).unwrap().bytes,
            out.bytes
        );
    }

    #[test]
    fn empty_block_on_empty_or_foreign_file_is_a_no_op() {
        assert_eq!(splice(b"", &[]).unwrap().bytes, b"");
        assert_eq!(splice(b"abc", &[]).unwrap().bytes, b"abc");
    }

    #[test]
    fn ambiguous_markers_refuse() {
        let cases = [
            format!("{BEGIN_MARKER}\nk\n"),
            format!("k\n{END_MARKER}\n"),
            format!("{END_MARKER}\nk\n{BEGIN_MARKER}\n"),
            format!("{BEGIN_MARKER}\n{END_MARKER}\n{BEGIN_MARKER}\n{END_MARKER}\n"),
            format!("{BEGIN_MARKER}\nk\n{BEGIN_MARKER}\n{END_MARKER}\n"),
            "# BEGIN YANTO MANAGED KEYS (edited)\nk\n".to_owned(),
        ];
        for c in cases {
            assert!(
                matches!(
                    splice(c.as_bytes(), &block(&["x"])),
                    Err(SpliceError::AmbiguousMarkers(_))
                ),
                "{c:?}"
            );
            assert!(outside_block(c.as_bytes()).is_err());
        }
    }

    #[test]
    fn counts_keys_inside_and_outside_with_options() {
        let k1 = ed25519(1, "a");
        let k2 = ed25519(2, "b");
        let file = format!(
            "# comment\n\ncommand=\"echo a b\",no-pty {k1}\ngarbage ssh-ed25519 notbase64\n{BEGIN_MARKER}\n{k2}\n{END_MARKER}\n"
        );
        assert_eq!(count_valid_keys(file.as_bytes()), 2);
        assert_eq!(count_valid_keys(b""), 0);
        assert_eq!(count_valid_keys(b"# only a comment\n"), 0);
        let outside = outside_block(file.as_bytes()).unwrap();
        assert_eq!(count_valid_keys(&outside), 1);
        assert_eq!(outside_fingerprints(&outside).len(), 1);
        assert_eq!(
            outside_fingerprints(&outside)[0],
            parse_public_key(&k1).unwrap().fingerprint
        );
    }
}
