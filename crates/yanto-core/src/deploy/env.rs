//! Minimal `.env` handling: parse, mask, patch, render. Values are secrets, so
//! everything user-visible goes through [`mask`].

use crate::error::CoreError;

/// Placeholder shown instead of a value. Writing it back means "keep the stored value".
pub const MASK: &str = "********";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A comment, blank line or anything unparsable, kept verbatim.
    Other(String),
    Var {
        key: String,
        value: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvFile {
    pub lines: Vec<Line>,
    /// 1-based numbers of lines that are neither blank, comment nor `KEY=VALUE`.
    pub invalid_lines: Vec<usize>,
}

pub fn valid_key(k: &str) -> bool {
    let mut chars = k.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && k.len() <= 255
}

pub fn parse(text: &str) -> EnvFile {
    let mut file = EnvFile::default();
    for (i, raw) in text.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            file.lines.push(Line::Other(raw.to_owned()));
            continue;
        }
        let body = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        match body.split_once('=') {
            Some((k, v)) if valid_key(k.trim()) => file.lines.push(Line::Var {
                key: k.trim().to_owned(),
                value: unquote(v.trim()),
            }),
            _ => {
                file.invalid_lines.push(i + 1);
                file.lines.push(Line::Other(raw.to_owned()));
            }
        }
    }
    file
}

fn unquote(v: &str) -> String {
    let bytes = v.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return v[1..v.len() - 1].to_owned();
        }
    }
    v.to_owned()
}

impl EnvFile {
    pub fn vars(&self) -> Vec<(&str, &str)> {
        self.lines
            .iter()
            .filter_map(|l| match l {
                Line::Var { key, value } => Some((key.as_str(), value.as_str())),
                Line::Other(_) => None,
            })
            .collect()
    }

    pub fn keys(&self) -> Vec<String> {
        self.vars().into_iter().map(|(k, _)| k.to_owned()).collect()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars()
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            match l {
                Line::Other(s) => out.push_str(s),
                Line::Var { key, value } => {
                    out.push_str(key);
                    out.push('=');
                    out.push_str(&quote_if_needed(value));
                }
            }
            out.push('\n');
        }
        out
    }

    /// Same text with every value replaced by [`MASK`] (comments kept).
    pub fn masked_text(&self) -> String {
        let masked = EnvFile {
            lines: self
                .lines
                .iter()
                .map(|l| match l {
                    Line::Var { key, .. } => Line::Var {
                        key: key.clone(),
                        value: MASK.to_owned(),
                    },
                    other => other.clone(),
                })
                .collect(),
            invalid_lines: Vec::new(),
        };
        masked.render()
    }
}

fn quote_if_needed(v: &str) -> String {
    if v.is_empty()
        || !v
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '#' | '"' | '\''))
    {
        return v.to_owned();
    }
    if v.contains('"') {
        format!("'{v}'")
    } else {
        format!("\"{v}\"")
    }
}

/// One requested change: `value = None` deletes the key.
#[derive(Debug, Clone)]
pub struct VarChange {
    pub key: String,
    pub value: Option<String>,
}

/// Apply changes in place. A value equal to [`MASK`] keeps the stored one.
pub fn patch(file: &mut EnvFile, changes: &[VarChange]) -> Result<(), CoreError> {
    for c in changes {
        if !valid_key(&c.key) {
            return Err(CoreError::validation(
                "key",
                format!("{:?} is not a valid variable name", c.key),
            ));
        }
        match &c.value {
            None => file
                .lines
                .retain(|l| !matches!(l, Line::Var { key, .. } if *key == c.key)),
            Some(v) if v == MASK => {}
            Some(v) => {
                if v.contains('\n') || v.contains('\r') {
                    return Err(CoreError::validation(
                        "value",
                        "values must be a single line",
                    ));
                }
                let mut found = false;
                for l in &mut file.lines {
                    if let Line::Var { key, value } = l
                        && *key == c.key
                    {
                        *value = v.clone();
                        found = true;
                    }
                }
                if !found {
                    file.lines.push(Line::Var {
                        key: c.key.clone(),
                        value: v.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Replace a whole file's text, keeping stored values where the new text carries [`MASK`].
pub fn merge_masked(existing: &EnvFile, new_text: &str) -> EnvFile {
    let mut file = parse(new_text);
    for l in &mut file.lines {
        if let Line::Var { key, value } = l
            && value == MASK
            && let Some(old) = existing.get(key)
        {
            *value = old.to_owned();
        }
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quotes_comments_and_invalid_lines() {
        let f = parse("# c\nA=1\nexport B=\"x y\"\nC='q'\nbroken line\n1BAD=x\n");
        assert_eq!(f.vars(), vec![("A", "1"), ("B", "x y"), ("C", "q")]);
        assert_eq!(f.invalid_lines, vec![5, 6]);
    }

    #[test]
    fn masking_never_leaks_values() {
        let f = parse("# note\nTOKEN=supersecret\nDB=postgres://u:p@h/db\n");
        let m = f.masked_text();
        assert!(!m.contains("supersecret") && !m.contains("postgres"));
        assert!(m.contains("TOKEN=********") && m.contains("# note"));
    }

    #[test]
    fn patch_updates_adds_deletes_and_preserves_masked() {
        let mut f = parse("A=1\nB=2\n");
        patch(
            &mut f,
            &[
                VarChange {
                    key: "A".into(),
                    value: Some(MASK.into()),
                },
                VarChange {
                    key: "B".into(),
                    value: None,
                },
                VarChange {
                    key: "C".into(),
                    value: Some("with space".into()),
                },
            ],
        )
        .unwrap();
        assert_eq!(f.render(), "A=1\nC=\"with space\"\n");
        assert!(
            patch(
                &mut f,
                &[VarChange {
                    key: "bad key".into(),
                    value: None
                }]
            )
            .is_err()
        );
        assert!(
            patch(
                &mut f,
                &[VarChange {
                    key: "X".into(),
                    value: Some("a\nb".into())
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn merge_keeps_stored_value_for_masked_placeholder() {
        let old = parse("A=real\nB=2\n");
        let merged = merge_masked(&old, "A=********\nB=3\nC=new\n");
        assert_eq!(merged.vars(), vec![("A", "real"), ("B", "3"), ("C", "new")]);
    }
}
