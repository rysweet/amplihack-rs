//! Bounded retention for `.claude/runtime/sessions.jsonl`.
//!
//! Issue #1272: the session log is append-only with no rotation. Every
//! launch appends a start record (which embeds the full `argv`, including
//! multi-kilobyte `-p` prompts) and an end record, and
//! [`crate::nesting::NestingDetector::detect`] parses the whole file on the
//! launch path to decide whether the new session is nested. Real logs on a
//! busy checkout reached **4.2 MB / 1600 lines**, costing **8.75 ms** of
//! JSON parsing per launch, growing without limit.
//!
//! This module bounds the file. Compaction is **not** blind truncation:
//!
//! - Every record for a session that has not ended is kept, unconditionally,
//!   while its process is alive — so nesting detection can never lose a
//!   session it would otherwise have reported.
//! - A record for an unfinished session whose process is gone is kept for
//!   [`DEAD_SESSION_GRACE_SECS`] before it becomes eligible for dropping;
//!   the reader already ignores such records, since it requires a live PID.
//! - The most recent [`RETAIN_TAIL_BYTES`] of history is kept verbatim for
//!   diagnostics, whatever it contains.
//! - **What is lost:** older records for sessions that have already ended.
//!   That loss is recorded in the file itself — compaction writes a
//!   `{"status":"compacted",...}` marker line naming how many lines and
//!   bytes went away and why — so the gap is explicit rather than silent.
//!
//! Writes go to a tempfile in the same directory and are `rename`d into
//! place, so a crash mid-compaction leaves the previous log intact. Callers
//! hold the log's advisory lock (see [`crate::session_tracker`]) across the
//! whole append-then-compact sequence, so a concurrent launch cannot append
//! into an inode that is about to be replaced.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Compact once the log grows past this size.
pub(crate) const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// Bytes of the most recent history kept verbatim for diagnostics.
pub(crate) const RETAIN_TAIL_BYTES: usize = 256 * 1024;

/// How long a start record for a session that never wrote an end record is
/// kept after its process disappears. The reader requires a live PID, so
/// these records cannot affect nesting detection; the grace period exists
/// only so a post-mortem can still see a crashed session.
pub(crate) const DEAD_SESSION_GRACE_SECS: f64 = 24.0 * 3600.0;

/// Tempfile used for the atomic rewrite.
const COMPACT_TMP_SUFFIX: &str = ".compact.tmp";

/// The subset of a session record that retention decisions depend on.
#[derive(Debug, Default, Deserialize)]
struct RetentionFields {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    pid: Option<u64>,
    #[serde(default)]
    start_time: Option<f64>,
}

fn parse_retention_fields(line: &str) -> Option<RetentionFields> {
    serde_json::from_str::<RetentionFields>(line).ok()
}

/// What a compaction did, for logging and for the in-file marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactionReport {
    pub(crate) dropped_lines: usize,
    pub(crate) dropped_bytes: usize,
    pub(crate) retained_lines: usize,
}

/// Index of the first line that falls inside the retained tail budget.
fn tail_start_index(lines: &[&str]) -> usize {
    let mut budget = RETAIN_TAIL_BYTES;
    let mut start = lines.len();
    for (index, line) in lines.iter().enumerate().rev() {
        let cost = line.len() + 1;
        if cost > budget {
            break;
        }
        budget -= cost;
        start = index;
    }
    start
}

/// Decide which lines survive compaction, preserving their original order.
///
/// `is_alive` is injected so tests can pin the decision without spawning
/// processes.
fn plan_retention<'a>(lines: &[&'a str], now: f64, is_alive: &dyn Fn(u32) -> bool) -> Vec<&'a str> {
    let mut terminated: HashSet<String> = HashSet::new();
    let mut parsed: Vec<Option<RetentionFields>> = Vec::with_capacity(lines.len());
    for line in lines {
        let fields = parse_retention_fields(line);
        if let Some(fields) = &fields
            && matches!(fields.status.as_deref(), Some("completed" | "crashed"))
            && let Some(session_id) = fields.session_id.as_deref()
        {
            terminated.insert(session_id.to_string());
        }
        parsed.push(fields);
    }

    let tail_start = tail_start_index(lines);
    let mut kept = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if index >= tail_start {
            kept.push(*line);
            continue;
        }
        let Some(fields) = &parsed[index] else {
            continue;
        };
        if fields.status.as_deref() != Some("active") {
            continue;
        }
        let Some(session_id) = fields.session_id.as_deref() else {
            continue;
        };
        if terminated.contains(session_id) {
            continue;
        }
        let alive = fields
            .pid
            .is_some_and(|pid| u32::try_from(pid).map(is_alive).unwrap_or(false));
        let recent = fields
            .start_time
            .is_some_and(|start| now - start <= DEAD_SESSION_GRACE_SECS);
        if alive || recent {
            kept.push(*line);
        }
    }
    kept
}

fn marker_line(report: &CompactionReport, now: f64) -> String {
    let marker = serde_json::json!({
        "status": "compacted",
        "compacted_at": now,
        "reason": format!(
            "sessions.jsonl exceeded {MAX_LOG_BYTES} bytes (issue #1272 bounded retention)"
        ),
        "dropped_lines": report.dropped_lines,
        "dropped_bytes": report.dropped_bytes,
        "retained_lines": report.retained_lines,
        "retained": format!(
            "every unfinished session plus the most recent {RETAIN_TAIL_BYTES} bytes of history"
        ),
        "discarded": "older records for sessions that had already ended",
    });
    marker.to_string()
}

fn now_secs_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

fn tmp_path(log_path: &Path) -> PathBuf {
    let mut name = log_path.as_os_str().to_os_string();
    name.push(COMPACT_TMP_SUFFIX);
    PathBuf::from(name)
}

/// Compact `log_path` if it has grown past [`MAX_LOG_BYTES`].
///
/// Returns `Ok(None)` when the log was already within budget. The caller is
/// expected to hold the log's advisory lock.
pub(crate) fn compact_if_needed(log_path: &Path) -> Result<Option<CompactionReport>> {
    let Ok(metadata) = fs::metadata(log_path) else {
        return Ok(None);
    };
    if metadata.len() <= MAX_LOG_BYTES {
        return Ok(None);
    }
    compact(log_path, &crate::nesting::process_is_alive)
}

fn compact(log_path: &Path, is_alive: &dyn Fn(u32) -> bool) -> Result<Option<CompactionReport>> {
    let content = fs::read_to_string(log_path)
        .with_context(|| format!("failed to read {} for compaction", log_path.display()))?;
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let now = now_secs_f64();
    let kept = plan_retention(&lines, now, is_alive);

    let original_bytes = content.len();
    let kept_bytes: usize = kept.iter().map(|line| line.len() + 1).sum();
    let report = CompactionReport {
        dropped_lines: lines.len() - kept.len(),
        dropped_bytes: original_bytes.saturating_sub(kept_bytes),
        retained_lines: kept.len(),
    };
    if report.dropped_lines == 0 {
        // Nothing to gain — every record is still load-bearing. Rewriting
        // would only churn the file and re-add a marker each launch.
        return Ok(None);
    }

    let mut out = String::with_capacity(kept_bytes + 512);
    out.push_str(&marker_line(&report, now));
    out.push('\n');
    for line in &kept {
        out.push_str(line);
        out.push('\n');
    }

    let tmp = tmp_path(log_path);
    fs::write(&tmp, out.as_bytes())
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    restrict_permissions(&tmp);
    fs::rename(&tmp, log_path).with_context(|| {
        format!(
            "failed to replace {} with compacted log",
            log_path.display()
        )
    })?;

    tracing::info!(
        "sessions.jsonl compacted: dropped {} ended-session lines ({} bytes); kept {} lines",
        report.dropped_lines,
        report.dropped_bytes,
        report.retained_lines
    );
    Ok(Some(report))
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn start_line(session_id: &str, pid: u32, start_time: f64, padding: usize) -> String {
        serde_json::json!({
            "pid": pid,
            "session_id": session_id,
            "launch_dir": "/work/project",
            "argv": ["amplihack", "claude", "-p", "x".repeat(padding)],
            "start_time": start_time,
            "is_auto_mode": false,
            "is_nested": false,
            "parent_session_id": Value::Null,
            "status": "active",
            "end_time": Value::Null,
        })
        .to_string()
    }

    fn end_line(session_id: &str, end_time: f64) -> String {
        serde_json::json!({
            "session_id": session_id,
            "status": "completed",
            "end_time": end_time,
        })
        .to_string()
    }

    fn never_alive(_pid: u32) -> bool {
        false
    }

    fn always_alive(_pid: u32) -> bool {
        true
    }

    /// Build a log big enough to trip the threshold: `pairs` completed
    /// sessions, each carrying a fat `argv` like the real thing.
    fn write_big_log(path: &Path, pairs: usize, padding: usize, base_time: f64) {
        let mut content = String::new();
        for index in 0..pairs {
            let session_id = format!("session-{index:016x}");
            content.push_str(&start_line(
                &session_id,
                1000 + index as u32,
                base_time,
                padding,
            ));
            content.push('\n');
            content.push_str(&end_line(&session_id, base_time + 1.0));
            content.push('\n');
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn small_log_is_left_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sessions.jsonl");
        write_big_log(&path, 3, 16, 1_000.0);
        let before = fs::read_to_string(&path).unwrap();

        assert_eq!(compact_if_needed(&path).unwrap(), None);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            before,
            "a log within budget must not be rewritten"
        );
    }

    #[test]
    fn oversized_log_is_bounded_and_records_what_it_dropped() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sessions.jsonl");
        // Sessions are old and their processes long gone.
        write_big_log(&path, 4_000, 512, 1_000.0);
        let before_len = fs::metadata(&path).unwrap().len();
        assert!(
            before_len > MAX_LOG_BYTES,
            "fixture must exceed the compaction threshold, got {before_len}"
        );

        let report = compact(&path, &never_alive)
            .unwrap()
            .expect("an oversized log must be compacted");

        let after_len = fs::metadata(&path).unwrap().len();
        assert!(
            after_len < before_len,
            "compaction must shrink the log ({before_len} -> {after_len})"
        );
        assert!(
            after_len as usize <= RETAIN_TAIL_BYTES + 4096,
            "compacted log must fit the retention budget, got {after_len}"
        );
        assert!(report.dropped_lines > 0 && report.dropped_bytes > 0);

        // The loss is explicit: the first line says so.
        let content = fs::read_to_string(&path).unwrap();
        let marker: Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(marker["status"], "compacted");
        assert_eq!(marker["dropped_lines"], report.dropped_lines);
        assert_eq!(marker["dropped_bytes"], report.dropped_bytes);
        assert!(
            marker["discarded"]
                .as_str()
                .unwrap()
                .contains("already ended"),
            "marker must name what was discarded: {marker}"
        );
    }

    #[test]
    fn compaction_never_drops_a_running_session() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sessions.jsonl");
        let base = 1_000.0;
        let mut content = String::new();
        // The live session's record is the very first line, so only the
        // active-session rule — not the tail budget — can save it.
        content.push_str(&start_line("session-still-running", 4242, base, 512));
        content.push('\n');
        for index in 0..4_000 {
            let session_id = format!("session-{index:016x}");
            content.push_str(&start_line(&session_id, 1000 + index, base, 512));
            content.push('\n');
            content.push_str(&end_line(&session_id, base + 1.0));
            content.push('\n');
        }
        fs::write(&path, &content).unwrap();

        compact(&path, &always_alive)
            .unwrap()
            .expect("oversized log must compact");

        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("session-still-running"),
            "a session that never wrote an end record must survive compaction"
        );
    }

    #[test]
    fn recent_unfinished_session_survives_even_with_a_dead_pid() {
        let now = now_secs_f64();
        let lines_owned = [
            start_line("session-crashed-recently", 999_999, now - 60.0, 8),
            start_line(
                "session-crashed-long-ago",
                999_998,
                now - 10.0 * 24.0 * 3600.0,
                8,
            ),
        ];
        // A log large enough that the tail budget cannot cover these two.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sessions.jsonl");
        let mut content = String::new();
        content.push_str(&lines_owned[0]);
        content.push('\n');
        content.push_str(&lines_owned[1]);
        content.push('\n');
        for index in 0..4_000 {
            let session_id = format!("session-{index:016x}");
            content.push_str(&start_line(&session_id, 1000 + index, now, 512));
            content.push('\n');
            content.push_str(&end_line(&session_id, now + 1.0));
            content.push('\n');
        }
        fs::write(&path, &content).unwrap();

        compact(&path, &never_alive).unwrap().expect("must compact");
        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("session-crashed-recently"),
            "an unfinished session inside the grace window must be kept"
        );
        assert!(
            !after.contains("session-crashed-long-ago"),
            "an unfinished session past the grace window with a dead PID must be dropped"
        );
    }

    #[test]
    fn compaction_is_atomic_and_leaves_no_tempfile() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sessions.jsonl");
        write_big_log(&path, 4_000, 512, 1_000.0);

        compact(&path, &never_alive).unwrap().expect("must compact");

        assert!(
            !tmp_path(&path).exists(),
            "the compaction tempfile must be renamed away, not left behind"
        );
        // Every surviving line must still be valid JSON.
        for line in fs::read_to_string(&path).unwrap().lines() {
            serde_json::from_str::<Value>(line).expect("compacted log must stay valid JSONL");
        }
    }

    #[test]
    fn a_log_of_only_live_sessions_is_not_rewritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sessions.jsonl");
        let now = now_secs_f64();
        let mut content = String::new();
        for index in 0..4_000 {
            content.push_str(&start_line(
                &format!("session-{index:016x}"),
                1000 + index,
                now,
                512,
            ));
            content.push('\n');
        }
        fs::write(&path, &content).unwrap();

        assert_eq!(
            compact(&path, &always_alive).unwrap(),
            None,
            "when nothing is droppable the log must be left exactly as it is"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
    }
}
