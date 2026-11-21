//! Deployment log text to safe HTML: everything is escaped, and SGR colors map to a fixed
//! whitelist of CSS classes (`ansi-30`..`ansi-37`, `ansi-90`..`ansi-97`, `ansi-1`). Logs are
//! untrusted (build output, git messages), so no other markup is ever produced.

use crate::dashboard::view::escape;

pub fn ansi_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut open = false;
    let mut chars = text.chars().peekable();
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut String| {
        if !plain.is_empty() {
            out.push_str(&escape(plain));
            plain.clear();
        }
    };
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
            continue;
        }
        if chars.peek() != Some(&'[') {
            continue; // a lone ESC is dropped
        }
        chars.next();
        let mut params = String::new();
        let mut final_byte = None;
        for c in chars.by_ref() {
            if c.is_ascii_digit() || c == ';' {
                params.push(c);
            } else {
                final_byte = Some(c);
                break;
            }
        }
        if final_byte != Some('m') {
            continue; // cursor movement, erase, ...: dropped
        }
        flush(&mut plain, &mut out);
        if open {
            out.push_str("</span>");
            open = false;
        }
        let classes: Vec<String> = params
            .split(';')
            .filter_map(|p| p.parse::<u8>().ok())
            .filter(|n| matches!(n, 1 | 30..=37 | 90..=97))
            .map(|n| format!("ansi-{n}"))
            .collect();
        if !classes.is_empty() {
            out.push_str(&format!("<span class=\"{}\">", classes.join(" ")));
            open = true;
        }
    }
    flush(&mut plain, &mut out);
    if open {
        out.push_str("</span>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup_and_whitelists_colors() {
        assert_eq!(
            ansi_to_html("<script>alert(1)</script> & \"x\""),
            "&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;x&quot;"
        );
        assert_eq!(
            ansi_to_html("\u{1b}[31mred\u{1b}[0m ok"),
            "<span class=\"ansi-31\">red</span> ok"
        );
        assert_eq!(
            ansi_to_html("\u{1b}[1;32mgo\u{1b}[0m"),
            "<span class=\"ansi-1 ansi-32\">go</span>"
        );
        // Unknown codes and cursor sequences produce no markup at all.
        assert_eq!(ansi_to_html("\u{1b}[38;5;200mx\u{1b}[2K\u{1b}[0m"), "x");
        assert_eq!(
            ansi_to_html("\u{1b}[31munterminated"),
            "<span class=\"ansi-31\">unterminated</span>"
        );
    }
}
