//! `resume/resume.log`: one line per hook call, including the calls that
//! end at the gate (`disabled`, `not-limit`, `bad-session-id`), a payload
//! that cannot be read (`bad-payload`) and a caught panic (`panicked`)
//! (issue #3172).
//!
//! ```text
//! 2026-10-21T14:13:20Z resume session=3f9a1c2b7e04 event=7d41e0a9c2b1 outcome=not-resumed live=spare cycle=swapped detail=no-tmux-pane error=rate_limit
//! ```
//!
//! Since #2085 a line may end with the swap record it was decided on, the
//! pane's tag and the keys sent (only the fields that apply):
//!
//! ```text
//! ... outcome=resumed-keys live=- cycle=- swap_seq=7 swap_live=spare pane=0c4e61a8d2f3 keys_sent=4 error=rate_limit
//! ```
//!
//! Every field is a hash prefix, a fixed outcome name, a checked profile
//! name, the ledger's cycle outcome, a number or an `io::ErrorKind` name,
//! and is reduced to `[A-Za-z0-9._:-]`, so no message text reaches the
//! file. Past `LOG_MAX_BYTES` the file is moved to `resume.log.1`.
//! Every line ends with `error=`, the payload's `error` value reduced by
//! [`reduce_error`] to `[a-z_]`, at most 32 characters, and empty when the
//! value is absent or not a string. Lines written before #3172 have no
//! such field and are read as `error=`.
//!
//! [`append`] waits for the resume lock without a timeout
//! ([`lock_exclusive`]) and returns why it wrote nothing, as a
//! [`LogWriteFailure`]. The caller records that reason with
//! [`mark_failure`] and on stderr; it never changes the outcome or the exit
//! code.
//!
//! The lock file is the guard's own, so nothing may be logged while this
//! process holds the guard lock on another descriptor: `flock` would wait on
//! itself. `run_with_affinity` keeps to this by logging only after
//! `commit` and `release` have returned.

use super::Outcome;
use amplihack_utils::secure_files::{
    check_private_dir, lock_exclusive, open_private_nofollow, write_private_atomic_nosync,
};
use std::fs;
use std::io::Write;
use std::path::Path;

/// The log is rotated before it would grow past this size.
pub const LOG_MAX_BYTES: u64 = 64 * 1024;
const LOG_FILE: &str = "resume.log";
const ROTATED_FILE: &str = "resume.log.1";
const LOCK_FILE: &str = "lock";
const FAILURE_MARKER: &str = "log-write-failed";
/// The longest reduced `error` value.
pub const MAX_ERROR_CHARS: usize = 32;
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
    /// How many resumes the session's record held when the decision was
    /// taken (issue #2912). Only a resume delivered with keys is recorded
    /// (issue #3104), so this counts delivered key resumes. Evidence for an
    /// operator reading this file; nothing branches on it.
    pub resumes: Option<u32>,
    /// Why a step failed, or why no key was sent: an `io::ErrorKind` name,
    /// `session-gone`, `no-tmux-pane` for `not-resumed`, one of
    /// [`super::keys::UNVERIFIED_REASONS`] for `pane-unverified`, or one of
    /// [`super::keys::UNRECOGNISED_REASONS`] for `screen-unrecognised`.
    pub detail: Option<String>,
    /// The payload's `error` value, already reduced by [`reduce_error`]
    /// (issue #3172). Empty when the value was absent or not a string.
    pub error: String,
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

/// The payload's `error` value as the log shows it: ASCII lowercased, every
/// character outside `[a-z_]` replaced by `_`, cut to
/// [`MAX_ERROR_CHARS`] characters; empty for an absent value. The only route
/// from payload text to the log, so no message text can reach a line.
pub fn reduce_error(error: Option<&str>) -> String {
    error
        .unwrap_or("")
        .chars()
        .take(MAX_ERROR_CHARS)
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() { c } else { '_' }
        })
        .collect()
}

/// The log line for `entry` at `now` (epoch seconds), newline included.
/// The fields added by issue #2085 appear only when set, so older lines
/// keep their form. `error=` always comes last (issue #3172), reduced again
/// here so a caller cannot put other text into it.
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
        ("resumes", entry.resumes.map(|n| n.to_string())),
        ("detail", entry.detail.clone()),
    ];
    for (key, value) in extra {
        if let Some(v) = value {
            line.push_str(&format!(" {key}={}", sanitize(&v)));
        }
    }
    line.push_str(" error=");
    line.push_str(&reduce_error(Some(&entry.error)));
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

/// Why [`append`] wrote no line. Each reason is a fixed `[a-z_]` token, so
/// it can be shown on stderr and in the `log-write-failed` marker without
/// carrying a path, an `io::Error` text or any payload value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogWriteFailure {
    /// The resume folder does not exist. It is never created to log.
    DirMissing,
    /// The resume folder is not a private directory of the current user.
    DirUnprivate,
    /// The lock file could not be opened or locked.
    LockFailed,
    /// The log file could not be opened safely (a symlink, a FIFO, another
    /// owner, a mode that is not private, or an I/O error).
    OpenFailed,
    /// The log could not be rotated or the line could not be written.
    WriteFailed,
}

impl LogWriteFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DirMissing => "dir_missing",
            Self::DirUnprivate => "dir_unprivate",
            Self::LockFailed => "lock_failed",
            Self::OpenFailed => "open_failed",
            Self::WriteFailed => "write_failed",
        }
    }

    /// Whether [`mark_failure`] can have left the `log-write-failed` marker
    /// for this reason. It never writes into a folder that is missing or
    /// not private, so only the lock, open and write failures have one.
    pub fn marker_written(self) -> bool {
        !matches!(self, Self::DirMissing | Self::DirUnprivate)
    }
}

/// Appends `line` to `dir/resume.log` under the resume lock, rotating it
/// first when it would pass `LOG_MAX_BYTES`. Never creates `dir`, never
/// writes through a symlink and never opens anything but a private regular
/// file, so a FIFO in place of the log or the lock cannot make it hang.
///
/// The lock is taken with [`lock_exclusive`], which waits as long as the
/// holder holds it, with no timeout. A holder that never releases the lock
/// therefore hangs the hook; the security requirements accept this risk.
/// The lock file is shared with the guard: the caller must not hold the
/// guard lock while it logs.
///
/// Returns why no line was written; the caller records that reason (see
/// [`mark_failure`]). A failure is never silent.
pub fn append(dir: &Path, line: &str) -> Result<(), LogWriteFailure> {
    if let Err(e) = check_private_dir(dir) {
        return Err(if e.kind() == std::io::ErrorKind::NotFound {
            LogWriteFailure::DirMissing
        } else {
            LogWriteFailure::DirUnprivate
        });
    }
    let lock_file =
        open_private_nofollow(&dir.join(LOCK_FILE)).map_err(|_| LogWriteFailure::LockFailed)?;
    // Blocking on purpose (issue #3172): a timed lock dropped lines without
    // a trace. Nothing in this process holds the guard lock here.
    let _lock = lock_exclusive(lock_file).map_err(|_| LogWriteFailure::LockFailed)?;
    let path = dir.join(LOG_FILE);
    let mut file = open_private_nofollow(&path).map_err(|_| LogWriteFailure::OpenFailed)?;
    let len = file
        .metadata()
        .map_err(|_| LogWriteFailure::OpenFailed)?
        .len();
    if len + line.len() as u64 > LOG_MAX_BYTES && len > 0 {
        drop(file);
        fs::rename(&path, dir.join(ROTATED_FILE)).map_err(|_| LogWriteFailure::WriteFailed)?;
        file = open_private_nofollow(&path).map_err(|_| LogWriteFailure::OpenFailed)?;
    }
    file.write_all(line.as_bytes())
        .map_err(|_| LogWriteFailure::WriteFailed)
}

/// Records that the line for `outcome` was not written, in the fixed-name
/// marker `dir/log-write-failed`: one line with the time, the outcome and
/// the reason, replacing the marker's previous content so it never grows.
/// Writes only into an existing private `dir`, never through a symlink and
/// without the resume lock. Its own failure has nowhere further to go; the
/// caller's stderr line still records the reason.
pub fn mark_failure(dir: &Path, now: i64, outcome: Outcome, reason: LogWriteFailure) {
    if check_private_dir(dir).is_err() {
        return;
    }
    let text = format!(
        "{} log-write-failed outcome={} reason={}\n",
        utc_timestamp(now),
        outcome.as_str(),
        reason.as_str()
    );
    // Ignored by design: see the doc comment. The stderr line is the record
    // of last resort.
    let _ = write_private_atomic_nosync(dir, FAILURE_MARKER, text.as_bytes());
}
