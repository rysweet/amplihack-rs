//! `resume/resume.log`: one line per decision after the gate passed the
//! auto-switch and limit checks.
//!
//! ```text
//! 2026-10-21T14:13:20Z resume session=3f9a1c2b7e04 event=7d41e0a9c2b1 outcome=resumed live=spare
//! ```
//!
//! Every field is a hash prefix, a fixed outcome name or a checked profile
//! name, and is reduced to `[A-Za-z0-9._:-]`, so no message text reaches
//! the file. Past `LOG_MAX_BYTES` the file is moved to `resume.log.1`.
//! Logging is best effort: a failure never changes the outcome or prints.

use super::Outcome;
use amplihack_utils::secure_files::{
    check_private_dir, lock_exclusive_timeout, open_private_nofollow,
};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// The log is rotated before it would grow past this size.
pub const LOG_MAX_BYTES: u64 = 64 * 1024;
const LOG_FILE: &str = "resume.log";
const ROTATED_FILE: &str = "resume.log.1";
const LOCK_FILE: &str = "lock";
const LOCK_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_FIELD_CHARS: usize = 64;

/// One decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// The first 12 hex digits of the session key.
    pub session: String,
    /// The first 12 hex digits of the event key.
    pub event: String,
    pub outcome: Outcome,
    /// The live profile the ledger named, if any.
    pub live: Option<String>,
}

/// `s` with every character outside `[A-Za-z0-9._:-]` replaced by `-`, cut
/// to 64 characters; `-` when empty.
pub fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .take(MAX_FIELD_CHARS)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if out.is_empty() { "-".to_string() } else { out }
}

/// The log line for `entry` at `now` (epoch seconds), newline included.
pub fn format_line(now: i64, entry: &LogEntry) -> String {
    format!(
        "{} resume session={} event={} outcome={} live={}\n",
        utc_timestamp(now),
        sanitize(&entry.session),
        sanitize(&entry.event),
        sanitize(entry.outcome.as_str()),
        sanitize(entry.live.as_deref().unwrap_or("")),
    )
}

/// `YYYY-MM-DDTHH:MM:SSZ` for epoch seconds (proleptic Gregorian, UTC).
fn utc_timestamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Appends `line` to `dir/resume.log` under the resume lock, rotating it
/// first when it would pass `LOG_MAX_BYTES`. Does nothing when `dir` is not
/// an existing private directory or when any step fails; never creates
/// `dir` and never writes through a symlink.
pub fn append(dir: &Path, line: &str) {
    if check_private_dir(dir).is_err() {
        return;
    }
    let Ok(lock_file) = open_private_nofollow(&dir.join(LOCK_FILE)) else {
        return;
    };
    let Ok(_lock) = lock_exclusive_timeout(lock_file, LOCK_TIMEOUT) else {
        return;
    };
    let path = dir.join(LOG_FILE);
    let Ok(mut file) = open_private_nofollow(&path) else {
        return;
    };
    let Ok(meta) = file.metadata() else {
        return;
    };
    if meta.len() + line.len() as u64 > LOG_MAX_BYTES && meta.len() > 0 {
        drop(file);
        if fs::rename(&path, dir.join(ROTATED_FILE)).is_err() {
            return;
        }
        file = match open_private_nofollow(&path) {
            Ok(f) => f,
            Err(_) => return,
        };
    }
    // Best effort by design: the hook's only output channel is reserved for
    // the resume message.
    let _ = file.write_all(line.as_bytes());
}
