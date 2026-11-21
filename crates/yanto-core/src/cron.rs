//! Cron Entries: pure domain logic (docs/rewrite/04 section 5).
//!
//! * [`Schedule`]: Yanto's own schedule validator and "next runs" calculator (5.3). Times are
//!   computed in UTC; the dashboard labels them as such (deviation recorded in 08-conventions).
//! * [`validate_entry`]: field-level validation of name, user, command and timeout.
//! * [`render_file`]: the generated `/etc/cron.d/yanto` text (5.3). The command never reaches the
//!   file; each line invokes `yanto cron-run <id>` so no user text can inject into the cron file.

use crate::error::{CoreError, ErrorKind, FieldError};

/// Name of the one cron file Yanto owns (D11). No dot: Debian cron skips dotted names.
pub const CRON_FILE_NAME: &str = "yanto";
/// Dotted (so never read by cron) temp name used while writing, inside `/etc/cron.d` (5.4): the
/// unit's `ReadWritePaths` only covers that dir, and a rename from `/etc` would cross its bind
/// mount.
pub const CRON_TEMP_NAME: &str = ".yanto-cron.tmp";
pub const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
pub const MAX_COMMAND_BYTES: usize = 4096;
pub const MAX_NAME_CHARS: usize = 100;
pub const MAX_TIMEOUT_S: i64 = 7 * 24 * 3600;
/// Tail of captured output kept per run (5.3).
pub const OUTPUT_TAIL_BYTES: usize = 64 * 1024;
pub const KEEP_RUNS: i64 = 50;
pub const KEEP_RUN_DAYS: i64 = 30;

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Spec {
    Reboot,
    Fields {
        minute: u64,
        hour: u32,
        dom: u32,
        mon: u16,
        /// Bit 0 = Sunday (7 is folded into 0).
        dow: u8,
        dom_star: bool,
        dow_star: bool,
    },
}

/// A validated schedule in canonical form (single spaces, lowercase macros).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    canonical: String,
    spec: Spec,
}

impl Schedule {
    /// Parse and validate a five-field expression or one of
    /// `@reboot @hourly @daily @weekly @monthly @yearly`.
    pub fn parse(input: &str) -> Result<Self, String> {
        let text = input.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return Err("schedule is empty".into());
        }
        if text.contains(|c: char| c.is_control()) {
            return Err("schedule contains control characters".into());
        }
        if let Some(name) = text.strip_prefix('@') {
            let name = name.to_ascii_lowercase();
            let fields = match name.as_str() {
                "reboot" => {
                    return Ok(Self {
                        canonical: "@reboot".into(),
                        spec: Spec::Reboot,
                    });
                }
                "hourly" => "0 * * * *",
                "daily" => "0 0 * * *",
                "weekly" => "0 0 * * 0",
                "monthly" => "0 0 1 * *",
                "yearly" => "0 0 1 1 *",
                _ => {
                    return Err(format!(
                        "unsupported macro @{name} (use @reboot, @hourly, @daily, @weekly, @monthly or @yearly)"
                    ));
                }
            };
            let mut s = Self::parse(fields)?;
            s.canonical = format!("@{name}");
            return Ok(s);
        }
        let parts: Vec<&str> = text.split(' ').collect();
        if parts.len() != 5 {
            return Err(format!(
                "expected 5 fields (minute hour day-of-month month day-of-week), found {}",
                parts.len()
            ));
        }
        let minute = field(parts[0], 0, 59, &[], "minute")?;
        let hour = field(parts[1], 0, 23, &[], "hour")?;
        let dom = field(parts[2], 1, 31, &[], "day-of-month")?;
        let mon = field(parts[3], 1, 12, &MONTHS, "month")?;
        // Day-of-week accepts 0-7; 7 is Sunday.
        let mut dow = field(parts[4], 0, 7, &DAYS, "day-of-week")?;
        if dow & (1 << 7) != 0 {
            dow = (dow & !(1 << 7)) | 1;
        }
        Ok(Self {
            canonical: text.clone(),
            spec: Spec::Fields {
                minute,
                hour: u32::try_from(hour).unwrap_or(0),
                dom: u32::try_from(dom).unwrap_or(0),
                mon: u16::try_from(mon).unwrap_or(0),
                dow: u8::try_from(dow).unwrap_or(0),
                dom_star: parts[2].starts_with('*'),
                dow_star: parts[4].starts_with('*'),
            },
        })
    }

    pub fn as_str(&self) -> &str {
        &self.canonical
    }

    pub fn is_reboot(&self) -> bool {
        self.spec == Spec::Reboot
    }

    /// Both day fields restricted: cron fires when EITHER matches (5.3 note).
    pub fn day_fields_use_or(&self) -> bool {
        matches!(self.spec, Spec::Fields { dom_star, dow_star, .. } if !dom_star && !dow_star)
    }

    /// The next `n` run times strictly after `after_ms` (unix ms, UTC). Empty for `@reboot`.
    pub fn next_runs(&self, after_ms: i64, n: usize) -> Vec<i64> {
        let Spec::Fields {
            minute,
            hour,
            dom,
            mon,
            dow,
            dom_star,
            dow_star,
        } = self.spec
        else {
            return Vec::new();
        };
        let after_min = after_ms.div_euclid(60_000);
        let first_day = after_min.div_euclid(1440);
        let mut out = Vec::with_capacity(n);
        // 8 years covers Feb 29 schedules.
        for day in first_day..first_day + 366 * 8 {
            if out.len() >= n {
                break;
            }
            let (_, m, d) = civil_from_days(day);
            let wd = u32::try_from((day + 4).rem_euclid(7)).unwrap_or(0);
            if mon & (1 << m) == 0 {
                continue;
            }
            let dom_ok = dom & (1 << d) != 0;
            let dow_ok = dow & (1 << wd) != 0;
            let day_ok = if dom_star || dow_star {
                dom_ok && dow_ok
            } else {
                dom_ok || dow_ok
            };
            if !day_ok {
                continue;
            }
            for h in 0..24u32 {
                if hour & (1 << h) == 0 {
                    continue;
                }
                for mi in 0..60u32 {
                    if minute & (1 << mi) == 0 {
                        continue;
                    }
                    let t = day * 1440 + i64::from(h) * 60 + i64::from(mi);
                    if t > after_min {
                        out.push(t * 60_000);
                        if out.len() >= n {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }
}

/// Parse one field into a bit set (bit `v` set when value `v` matches).
fn field(text: &str, min: u32, max: u32, names: &[&str], what: &str) -> Result<u64, String> {
    let bad = |m: &str| format!("{what}: {m}");
    if text.is_empty() {
        return Err(bad("empty field"));
    }
    let mut bits: u64 = 0;
    for item in text.split(',') {
        if item.is_empty() {
            return Err(bad("empty list item"));
        }
        let (range, step) = match item.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s.parse().map_err(|_| bad(&format!("invalid step {s:?}")))?;
                if step == 0 {
                    return Err(bad("step must not be 0"));
                }
                (r, Some(step))
            }
            None => (item, None),
        };
        let (lo, hi) = if range == "*" {
            (min, if names.len() == 7 { 6 } else { max })
        } else if let Some((a, b)) = range.split_once('-') {
            let (a, b) = (
                value(a, min, max, names, what)?,
                value(b, min, max, names, what)?,
            );
            if a > b {
                return Err(bad(&format!("descending range {range:?}")));
            }
            (a, b)
        } else {
            if step.is_some() {
                return Err(bad("a step needs * or a range (for example */5 or 0-30/5)"));
            }
            let v = value(range, min, max, names, what)?;
            (v, v)
        };
        let step = step.unwrap_or(1);
        let mut v = lo;
        while v <= hi {
            bits |= 1 << v;
            v += step;
        }
    }
    Ok(bits)
}

fn value(s: &str, min: u32, max: u32, names: &[&str], what: &str) -> Result<u32, String> {
    let lower = s.to_ascii_lowercase();
    if let Some(i) = names.iter().position(|n| *n == lower) {
        // Month names start at 1, day names at 0.
        let base = if names.len() == 12 { 1 } else { 0 };
        return Ok(u32::try_from(i).unwrap_or(0) + base);
    }
    let v: u32 = s
        .parse()
        .map_err(|_| format!("{what}: invalid value {s:?}"))?;
    if v < min || v > max {
        return Err(format!("{what}: {v} is outside {min}-{max}"));
    }
    Ok(v)
}

/// (year, month 1-12, day 1-31) from days since 1970-01-01 (Howard Hinnant).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Unvalidated entry input (create/update).
#[derive(Debug, Clone)]
pub struct EntryInput {
    pub name: String,
    pub schedule: String,
    pub run_as_user: String,
    pub command: String,
    pub overlap: Overlap,
    pub timeout_s: Option<i64>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Overlap {
    Allow,
    Skip,
}

impl Overlap {
    pub fn as_str(self) -> &'static str {
        match self {
            Overlap::Allow => "allow",
            Overlap::Skip => "skip",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Overlap::Allow),
            "skip" => Some(Overlap::Skip),
            _ => None,
        }
    }
}

/// An entry that passed [`validate_entry`] (schedule canonicalized, name trimmed).
#[derive(Debug, Clone)]
pub struct ValidEntry {
    pub name: String,
    pub schedule: Schedule,
    pub run_as_user: String,
    pub command: String,
    pub overlap: Overlap,
    pub timeout_s: Option<i64>,
    pub enabled: bool,
}

/// Validate every field; all problems are reported together as field errors (5.5).
pub fn validate_entry(input: &EntryInput) -> Result<ValidEntry, CoreError> {
    let mut errs: Vec<FieldError> = Vec::new();
    let mut fail = |field: &str, message: String| {
        errs.push(FieldError {
            field: field.into(),
            message,
        });
    };
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        fail(
            "name",
            format!("name must be 1-{MAX_NAME_CHARS} characters"),
        );
    } else if name.chars().any(char::is_control) {
        fail("name", "name must not contain control characters".into());
    }
    let schedule = match Schedule::parse(&input.schedule) {
        Ok(s) => Some(s),
        Err(m) => {
            fail("schedule", m);
            None
        }
    };
    if !valid_unix_name(&input.run_as_user) {
        fail("run_as_user", "not a valid Unix user name".into());
    }
    let cmd = &input.command;
    if cmd.trim().is_empty() {
        fail("command", "command is empty".into());
    } else if cmd.contains(['\n', '\r']) {
        fail("command", "command must be a single line".into());
    } else if cmd.contains('\0') {
        fail("command", "command must not contain NUL".into());
    } else if cmd.len() > MAX_COMMAND_BYTES {
        fail(
            "command",
            format!("command is longer than {MAX_COMMAND_BYTES} bytes"),
        );
    }
    if let Some(t) = input.timeout_s
        && !(1..=MAX_TIMEOUT_S).contains(&t)
    {
        fail(
            "timeout_s",
            format!("timeout must be 1-{MAX_TIMEOUT_S} seconds"),
        );
    }
    if !errs.is_empty() {
        let message = errs
            .iter()
            .map(|e| format!("{}: {}", e.field, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(CoreError::new(ErrorKind::Validation(errs), message));
    }
    Ok(ValidEntry {
        name,
        schedule: schedule.expect("checked above"),
        run_as_user: input.run_as_user.clone(),
        command: input.command.clone(),
        overlap: input.overlap,
        timeout_s: input.timeout_s,
        enabled: input.enabled,
    })
}

/// `useradd`-style names: letters, digits, `_`, `-`, `.`; not starting with `-`; at most 32 bytes.
pub fn valid_unix_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Debian cron only reads `/etc/cron.d` files named with letters, digits, `_` and `-` (5.2).
pub fn valid_cron_filename(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// One row as the renderer needs it.
#[derive(Debug, Clone)]
pub struct RenderEntry<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub schedule: &'a str,
    pub enabled: bool,
}

/// Render `/etc/cron.d/yanto` (5.3). `yanto_bin` must be an absolute path without whitespace,
/// quotes or `%`. The result always ends with a newline and every command is `yanto cron-run <id>`.
pub fn render_file(entries: &[RenderEntry<'_>], yanto_bin: &str) -> Result<String, CoreError> {
    if !yanto_bin.starts_with('/')
        || yanto_bin
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '%' | '"' | '\'' | '\\'))
    {
        return Err(CoreError::new(
            ErrorKind::Internal,
            "the yanto binary path cannot be used in a cron file",
        ));
    }
    let mut out = String::new();
    out.push_str("# Managed by Yanto -- edits are overwritten (see the Cron page).\n");
    out.push_str("SHELL=/bin/sh\n");
    out.push_str(&format!("PATH={DEFAULT_PATH}\n"));
    out.push_str("MAILTO=\"\"\n");
    for e in entries {
        out.push('\n');
        let name: String = e
            .name
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        out.push_str(&format!("# {name}\n"));
        if e.enabled {
            out.push_str(&format!(
                "{} root {yanto_bin} cron-run {}\n",
                e.schedule, e.id
            ));
        } else {
            out.push_str(&format!("# disabled: {}\n", e.id));
        }
    }
    debug_assert!(out.ends_with('\n'));
    Ok(out)
}

/// Last [`OUTPUT_TAIL_BYTES`] of `bytes` as UTF-8 (lossy); a cut multi-byte char becomes U+FFFD.
pub fn output_tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(OUTPUT_TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-01-01T00:00:00Z is a Thursday.
    const T0: i64 = 1_767_225_600_000;

    fn runs(s: &str, n: usize) -> Vec<i64> {
        Schedule::parse(s).unwrap().next_runs(T0, n)
    }

    #[test]
    fn rejects_bad_schedules() {
        for bad in [
            "61 * * * *",
            "*/0 * * * *",
            "* * * *",
            "* * * * * *",
            "",
            "5-1 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "* * * * 8",
            ",5 * * * *",
            "5/10 * * * *",
            "@bogus",
            "@annually",
            "a * * * *",
        ] {
            assert!(Schedule::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn accepts_valid_schedules_and_names() {
        for ok in [
            "* * * * *",
            "*/15 0-6 1,15 jan-mar mon-fri",
            "0 0 * * 7",
            "0 12 * JAN SUN",
            "@daily",
            "@REBOOT",
            "  5   4  *  *  * ",
        ] {
            assert!(Schedule::parse(ok).is_ok(), "{ok:?} should be accepted");
        }
        assert_eq!(
            Schedule::parse("  5   4 * * * ").unwrap().as_str(),
            "5 4 * * *"
        );
        assert_eq!(Schedule::parse("@DAILY").unwrap().as_str(), "@daily");
    }

    #[test]
    fn next_runs_basic() {
        let r = runs("*/15 * * * *", 3);
        assert_eq!(
            r,
            vec![T0 + 15 * 60_000, T0 + 30 * 60_000, T0 + 45 * 60_000]
        );
        // Strictly after: asking at an exact match skips it.
        assert_eq!(runs("0 0 * * *", 1), vec![T0 + 86_400_000]);
        assert_eq!(runs("@reboot", 3), Vec::<i64>::new());
        assert_eq!(runs("30 4 * * *", 1), vec![T0 + (4 * 60 + 30) * 60_000]);
    }

    #[test]
    fn dow_names_and_sunday_seven() {
        // First Sunday after Thu 2026-01-01 is 2026-01-04.
        let sun = T0 + 3 * 86_400_000;
        assert_eq!(runs("0 0 * * 7", 1), vec![sun]);
        assert_eq!(runs("0 0 * * 0", 1), vec![sun]);
        assert_eq!(runs("0 0 * * sun", 1), vec![sun]);
        // Monday-Friday: Friday 2026-01-02 comes next at 00:00 + one day.
        assert_eq!(runs("0 0 * * mon-fri", 1), vec![T0 + 86_400_000]);
    }

    #[test]
    fn dom_dow_or_semantics() {
        // Both restricted: the 15th OR any Monday. Next is Monday 2026-01-05, before the 15th.
        let s = Schedule::parse("0 0 15 * 1").unwrap();
        assert!(s.day_fields_use_or());
        assert_eq!(s.next_runs(T0, 1), vec![T0 + 4 * 86_400_000]);
        // One side `*`: AND (the day-of-month field is the only restriction).
        let s = Schedule::parse("0 0 15 * *").unwrap();
        assert!(!s.day_fields_use_or());
        assert_eq!(s.next_runs(T0, 1), vec![T0 + 14 * 86_400_000]);
    }

    #[test]
    fn month_rollover_and_leap_day() {
        let r = Schedule::parse("0 0 29 2 *").unwrap().next_runs(T0, 1);
        // 2028-02-29T00:00:00Z
        assert_eq!(r, vec![1_835_395_200_000]);
        let r = Schedule::parse("@monthly").unwrap().next_runs(T0, 2);
        assert_eq!(r, vec![T0 + 31 * 86_400_000, T0 + 59 * 86_400_000]);
    }

    #[test]
    fn steps_over_ranges() {
        let r = runs("0-10/5 0 * * *", 4);
        assert_eq!(
            r,
            vec![
                T0 + 5 * 60_000,
                T0 + 10 * 60_000,
                T0 + 86_400_000,
                T0 + 86_400_000 + 5 * 60_000
            ]
        );
    }

    fn input() -> EntryInput {
        EntryInput {
            name: "nightly backup".into(),
            schedule: "0 3 * * *".into(),
            run_as_user: "root".into(),
            command: "yanto backup yanto --keep-last 14".into(),
            overlap: Overlap::Skip,
            timeout_s: Some(600),
            enabled: true,
        }
    }

    #[test]
    fn entry_validation_reports_each_field() {
        assert!(validate_entry(&input()).is_ok());
        let mut bad = input();
        bad.name = String::new();
        bad.schedule = "61 * * * *".into();
        bad.run_as_user = "-x".into();
        bad.command = "a\nb".into();
        bad.timeout_s = Some(0);
        let e = validate_entry(&bad).unwrap_err();
        let ErrorKind::Validation(fields) = e.kind else {
            panic!("not a validation error")
        };
        let names: Vec<_> = fields.iter().map(|f| f.field.as_str()).collect();
        assert_eq!(
            names,
            ["name", "schedule", "run_as_user", "command", "timeout_s"]
        );
        let mut long = input();
        long.command = "x".repeat(MAX_COMMAND_BYTES + 1);
        assert!(validate_entry(&long).is_err());
        let mut nul = input();
        nul.command = "a\0b".into();
        assert!(validate_entry(&nul).is_err());
        // `%` in the command is fine: it never reaches the cron file.
        let mut pct = input();
        pct.command = "date +%F".into();
        assert!(validate_entry(&pct).is_ok());
    }

    #[test]
    fn rendered_file_wraps_every_entry_and_ends_with_newline() {
        let text = render_file(
            &[
                RenderEntry {
                    id: "crn_a",
                    name: "one",
                    schedule: "*/5 * * * *",
                    enabled: true,
                },
                RenderEntry {
                    id: "crn_b",
                    name: "two\nrm -rf /",
                    schedule: "@daily",
                    enabled: false,
                },
            ],
            "/usr/local/bin/yanto",
        )
        .unwrap();
        assert!(text.ends_with('\n'));
        assert!(text.contains("SHELL=/bin/sh\n") && text.contains("MAILTO=\"\"\n"));
        assert!(text.contains("*/5 * * * * root /usr/local/bin/yanto cron-run crn_a\n"));
        assert!(text.contains("# disabled: crn_b\n"));
        // A newline smuggled into a name cannot start a new line.
        assert!(!text.lines().any(|l| l.starts_with("rm ")));
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let is_env = line.starts_with("SHELL=")
                || line.starts_with("PATH=")
                || line.starts_with("MAILTO=");
            assert!(is_env || line.contains(" root /usr/local/bin/yanto cron-run crn_"));
        }
        assert!(render_file(&[], "/opt/my yanto/yanto").is_err());
        assert!(render_file(&[], "relative/yanto").is_err());
        assert!(render_file(&[], "/opt/y%anto").is_err());
    }

    #[test]
    fn file_and_user_name_rules() {
        assert!(valid_cron_filename(CRON_FILE_NAME));
        assert!(!valid_cron_filename("yanto.cron"));
        assert!(!valid_cron_filename(CRON_TEMP_NAME));
        assert!(valid_unix_name("deploy-1"));
        assert!(!valid_unix_name("a b"));
        assert!(!valid_unix_name(""));
    }

    #[test]
    fn output_tail_keeps_the_end() {
        let big = vec![b'a'; OUTPUT_TAIL_BYTES + 10];
        assert_eq!(output_tail(&big).len(), OUTPUT_TAIL_BYTES);
        assert_eq!(output_tail(b"hello"), "hello");
        let mut v = vec![b'x'; 10];
        v.extend_from_slice(&[0xff, b'z']);
        assert!(output_tail(&v).ends_with("\u{fffd}z"));
    }
}
