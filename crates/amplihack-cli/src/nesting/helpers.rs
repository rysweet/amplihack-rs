//! Nesting detection helpers — session log parsing and process checks.

use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use amplihack_types::ProjectDirs;

use super::NestingResult;

const SESSION_FILE_NAME: &str = "session.json";
pub(super) const STALE_THRESHOLD: Duration = Duration::from_secs(3600); // 1 hour

/// Session file format stored in `~/.claude/runtime/session.json`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct SessionFile {
    pub(super) session_id: String,
    pub(super) pid: u32,
    pub(super) timestamp: u64,
}

pub(super) fn detect_from_sessions_log(current_dir: &Path) -> Option<NestingResult> {
    let log_path = ProjectDirs::from_root(current_dir).sessions_log_file();
    let content = std::fs::read_to_string(log_path).ok()?;
    if content.trim().is_empty() {
        return None;
    }

    let resolved_current_dir = resolve_path(current_dir);
    let mut sessions: BTreeMap<String, Value> = BTreeMap::new();
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(session_id) = entry
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };

        match entry.get("status").and_then(Value::as_str) {
            Some("completed" | "crashed") => {
                sessions.remove(&session_id);
            }
            Some("active") => {
                sessions.insert(session_id, entry);
            }
            _ => {}
        }
    }

    for entry in sessions.values().rev() {
        let Some(pid) = entry.get("pid").and_then(Value::as_u64) else {
            continue;
        };
        if !is_process_alive(pid as u32) {
            continue;
        }
        let Some(launch_dir) = entry.get("launch_dir").and_then(Value::as_str) else {
            continue;
        };
        if resolve_path(Path::new(launch_dir)) != resolved_current_dir {
            continue;
        }
        let session_id = entry.get("session_id").and_then(Value::as_str)?.to_string();
        let depth = entry
            .get("parent_session_id")
            .and_then(Value::as_str)
            .map(|_| 2)
            .unwrap_or(1);
        return Some(NestingResult::Nested { session_id, depth });
    }

    None
}

pub(super) fn resolve_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn session_file_path() -> Option<PathBuf> {
    env::var("HOME").ok().map(|h| {
        PathBuf::from(h)
            .join(".claude")
            .join("runtime")
            .join(SESSION_FILE_NAME)
    })
}

/// Check if a process with the given PID is alive.
#[cfg(unix)]
pub(super) fn is_process_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) is a standard POSIX signal-safe way to check if
    // a process exists without actually sending a signal.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Check if a process with the given PID is alive (non-Unix fallback).
///
/// # Limitation
///
/// On non-Unix platforms (primarily Windows) this always returns `true`,
/// meaning stale nesting-guard entries will never be detected. To implement
/// proper detection on Windows, use `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, ...)`
/// and check the result with `GetExitCodeProcess`.  This is left as a future
/// improvement since amplihack does not yet actively target Windows.
#[cfg(not(unix))]
pub(super) fn is_process_alive(_pid: u32) -> bool {
    // On non-Unix, assume alive (conservative)
    true
}

pub(super) fn is_stale(timestamp: u64) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    now.saturating_sub(timestamp) > STALE_THRESHOLD.as_secs()
}
