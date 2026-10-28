//! What a detached run's log says about it: the `recipe run` log pointer
//! lines (issue, PR, start time, exit code) and the tail of its output.
//!
//! Only the first and last 64 KiB of a log are read. The task description in
//! the pointer is never deserialised, and the tail is only ever classified
//! (usage limit or not); no log text reaches the output.

use std::fmt;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::commands::recipe::run::correlation::LOG_POINTER_PREFIX;

/// Bytes read from each end of a log.
pub(crate) const EDGE_BYTES: u64 = 64 * 1024;
/// Pointer lines longer than this are skipped.
const MAX_LINE: usize = 16 * 1024;

#[derive(Clone, Default, PartialEq)]
pub(crate) struct LogInfo {
    pub issue: Option<u64>,
    pub pr: Option<u64>,
    /// The `early` pointer's timestamp.
    pub started: Option<DateTime<Utc>>,
    /// The last `final` pointer's exit code.
    pub exit_code: Option<i32>,
    /// The last 64 KiB of the log, for the usage-limit classifier only.
    pub tail: Vec<u8>,
}

// The tail is log output: never print it, even in a debug dump.
impl fmt::Debug for LogInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogInfo")
            .field("issue", &self.issue)
            .field("pr", &self.pr)
            .field("started", &self.started)
            .field("exit_code", &self.exit_code)
            .field("tail_bytes", &self.tail.len())
            .finish()
    }
}

/// The pointer fields this reader needs. Unknown fields, the task text among
/// them, are skipped by serde without being kept.
#[derive(Deserialize)]
struct Pointer {
    event: Option<String>,
    timestamp: Option<Value>,
    issue_number: Option<Value>,
    pr_number: Option<Value>,
    exit_code: Option<Value>,
}

/// Read `log` (a private regular file; anything else reads as empty).
pub(crate) fn read(log: &Path) -> LogInfo {
    let Some((mut file, meta)) = super::meta::open_private(log) else {
        return LogInfo::default();
    };
    let len = meta.len();
    let (head, tail) = if len <= 2 * EDGE_BYTES {
        let mut all = Vec::new();
        if file.read_to_end(&mut all).is_err() {
            return LogInfo::default();
        }
        let tail_start = all.len().saturating_sub(EDGE_BYTES as usize);
        let tail = all[tail_start..].to_vec();
        (all, tail)
    } else {
        let mut head = Vec::new();
        if (&mut file).take(EDGE_BYTES).read_to_end(&mut head).is_err() {
            return LogInfo::default();
        }
        // Drop the cut-off last line of the head.
        if let Some(end) = head.iter().rposition(|&b| b == b'\n') {
            head.truncate(end + 1);
        }
        let mut tail = Vec::new();
        if file.seek(SeekFrom::Start(len - EDGE_BYTES)).is_err()
            || file.take(EDGE_BYTES).read_to_end(&mut tail).is_err()
        {
            return LogInfo::default();
        }
        // Drop the cut-off first line of the tail.
        let start = tail.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1);
        let tail = tail[start..].to_vec();
        let mut both = head;
        both.extend_from_slice(&tail);
        (both, tail)
    };
    let mut info = LogInfo::default();
    let mut seen_early = false;
    for line in head.split(|&b| b == b'\n') {
        if line.len() > MAX_LINE {
            continue;
        }
        let Some(json) = line.strip_prefix(LOG_POINTER_PREFIX.as_bytes()) else {
            continue;
        };
        let Ok(p) = serde_json::from_slice::<Pointer>(json) else {
            continue;
        };
        if info.issue.is_none() {
            info.issue = p.issue_number.as_ref().and_then(number);
        }
        if info.pr.is_none() {
            info.pr = p.pr_number.as_ref().and_then(number);
        }
        match p.event.as_deref() {
            Some("early") if !seen_early => {
                seen_early = true;
                info.started = p
                    .timestamp
                    .as_ref()
                    .and_then(Value::as_str)
                    .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| t.to_utc());
            }
            Some("final") => {
                info.exit_code = p
                    .exit_code
                    .as_ref()
                    .and_then(Value::as_i64)
                    .and_then(|c| i32::try_from(c).ok());
            }
            _ => {}
        }
    }
    info.tail = tail;
    info
}

/// An issue or PR number given as a number or a string (`"1710"`, `"#1710"`).
fn number(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().trim_start_matches('#').parse().ok(),
        _ => None,
    }
}
