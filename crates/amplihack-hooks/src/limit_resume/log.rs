//! `resume/resume.log`: bounded receipt and lifecycle diagnostics plus
//! backwards-compatible decisions after the auto-switch and limit gates.
//!
//! ```text
//! 2026-10-21T14:13:20Z resume session=3f9a1c2b7e04 event=7d41e0a9c2b1 outcome=resumed live=spare cycle=swapped
//! ```
//!
//! Since #2085 a line may end with the swap record it was decided on, the
//! pane's tag and the keys sent (only the fields that apply):
//!
//! ```text
//! ... outcome=resumed-keys live=- cycle=- swap_seq=7 swap_live=spare pane=0c4e61a8d2f3 keys_sent=4
//! ```
//!
//! Every field is a hash prefix, a fixed outcome name, a checked profile
//! name, the ledger's cycle outcome, a number or an `io::ErrorKind` name,
//! and is reduced to `[A-Za-z0-9._:-]`, so no message text reaches the
//! file. The allowlisted error string instead uses reversible percent encoding
//! of a UTF-8-safe prefix of at most 256 bytes, with explicit truncation.
//! No other payload text is logged. Past `LOG_MAX_BYTES` the file is moved to `resume.log.1`.
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

/// Allowlisted observation of the error field; never serializes objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorObservation {
    Missing,
    InvalidType,
    Value { encoded: String, truncated: bool },
}

impl ErrorObservation {
    pub fn from_value(value: Option<&serde_json::Value>) -> Self {
        match value {
            None => Self::Missing,
            Some(serde_json::Value::String(value)) => {
                let mut end = value.len().min(256);
                while !value.is_char_boundary(end) {
                    end -= 1;
                }
                let mut encoded = String::new();
                for byte in value[..end].bytes() {
                    if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':') {
                        encoded.push(char::from(byte));
                    } else {
                        const HEX: &[u8; 16] = b"0123456789ABCDEF";
                        encoded.push('%');
                        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
                        encoded.push(char::from(HEX[usize::from(byte & 15)]));
                    }
                }
                Self::Value {
                    encoded,
                    truncated: end < value.len(),
                }
            }
            Some(_) => Self::InvalidType,
        }
    }
}

/// Fixed lifecycle stages, separate from the backwards-compatible outcomes.
#[derive(Debug, Clone, Copy)]
pub enum Stage {
    Receipt,
    Filtered,
    WaitingArmed,
    SwapJudged,
    ContinuationAttempt,
    ContinuationResult,
}
impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Receipt => "receipt",
            Self::Filtered => "filtered",
            Self::WaitingArmed => "waiting-armed",
            Self::SwapJudged => "swap-judged",
            Self::ContinuationAttempt => "continuation-attempt",
            Self::ContinuationResult => "continuation-result",
        }
    }
}

/// Bounded diagnostic fields. Dispositions are fixed strings at call sites.
pub struct Diagnostic {
    /// Bounded, payload-independent identifier shared by this invocation’s stages.
    pub invocation: String,
    pub session: Option<String>,
    pub event: Option<String>,
    pub stage: Stage,
    pub disposition: &'static str,
    pub error: Option<ErrorObservation>,
}

pub fn format_diagnostic(now: i64, entry: &Diagnostic) -> String {
    let mut line = format!(
        "{} resume stage={} disposition={} invocation={}",
        utc_timestamp(now),
        entry.stage.as_str(),
        sanitize(entry.disposition),
        sanitize(&entry.invocation)
    );
    for (key, value) in [("session", &entry.session), ("event", &entry.event)] {
        if let Some(value) = value {
            line.push_str(&format!(" {key}={}", sanitize(value)));
        }
    }
    match &entry.error {
        Some(ErrorObservation::Missing) => line.push_str(" error_kind=missing"),
        Some(ErrorObservation::InvalidType) => line.push_str(" error_kind=invalid-type"),
        Some(ErrorObservation::Value { encoded, truncated }) => {
            line.push_str(&format!(
                " error_kind=value error={encoded} truncated={truncated}"
            ));
        }
        None => {}
    }
    line.push('\n');
    line
}

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
    /// The ledger's cycle outcome (`swapped`, `stayed`, ...), if any.
    pub cycle: Option<String>,
    /// The `seq` of the swap record decided on (issue #2085).
    pub swap_seq: Option<u64>,
    /// The login that swap made live.
    pub swap_live: Option<String>,
    /// The pane's log tag ([`super::keys::Pane::log_tag`]), never its id.
    pub pane: Option<String>,
    /// How many resume keys were sent.
    pub keys_sent: Option<u8>,
    /// Why a step failed: an `io::ErrorKind` name or `session-gone`.
    pub detail: Option<String>,
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
/// The fields added by issue #2085 appear only when set, so older lines
/// keep their form.
pub fn format_line(now: i64, entry: &LogEntry) -> String {
    let mut line = format!(
        "{} resume session={} event={} outcome={} live={} cycle={}",
        utc_timestamp(now),
        sanitize(&entry.session),
        sanitize(&entry.event),
        sanitize(entry.outcome.as_str()),
        sanitize(entry.live.as_deref().unwrap_or("")),
        sanitize(entry.cycle.as_deref().unwrap_or("")),
    );
    let extra = [
        ("swap_seq", entry.swap_seq.map(|n| n.to_string())),
        ("swap_live", entry.swap_live.clone()),
        ("pane", entry.pane.clone()),
        ("keys_sent", entry.keys_sent.map(|n| n.to_string())),
        ("detail", entry.detail.clone()),
    ];
    for (key, value) in extra {
        if let Some(v) = value {
            line.push_str(&format!(" {key}={}", sanitize(&v)));
        }
    }
    line.push('\n');
    line
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
