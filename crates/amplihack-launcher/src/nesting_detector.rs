//! Nesting detection for amplihack sessions.
//!
//! Matches Python `amplihack/launcher/nesting_detector.py`:
//! - Detect nested sessions via JSONL log
//! - Detect amplihack source repository
//! - Determine staging requirements

use std::fs;
use std::path::{Path, PathBuf};

use crate::session_tracker::SessionEntry;

/// Results from nesting detection.
#[derive(Debug, Clone)]
pub struct NestingResult {
    pub is_nested: bool,
    pub in_source_repo: bool,
    pub parent_session_id: Option<String>,
    pub active_session: Option<SessionEntry>,
    pub requires_staging: bool,
}

/// Detect nested amplihack sessions and source repo execution.
pub struct NestingDetector {
    runtime_log: PathBuf,
}

impl Default for NestingDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl NestingDetector {
    pub fn new() -> Self {
        Self {
            runtime_log: PathBuf::from(".claude/runtime/sessions.jsonl"),
        }
    }

    /// Override the runtime log path (for testing).
    pub fn with_runtime_log(mut self, path: PathBuf) -> Self {
        self.runtime_log = path;
        self
    }

    /// Main detection — checks all conditions.
    pub fn detect_nesting(&self, cwd: &Path, argv: &[String]) -> NestingResult {
        let in_source_repo = Self::is_amplihack_source_repo(cwd);
        let active_session = self.find_active_session(cwd);

        let is_nested = active_session.is_some();
        let parent_session_id = active_session
            .as_ref()
            .map(|s| s.session_id.clone());

        // Auto-mode always stages
        let is_auto_mode = argv.iter().any(|a| a == "--auto");
        let requires_staging = is_auto_mode;

        NestingResult {
            is_nested,
            in_source_repo,
            parent_session_id,
            active_session,
            requires_staging,
        }
    }

    /// Check if running in the amplihack source repository.
    pub fn is_amplihack_source_repo(cwd: &Path) -> bool {
        let pyproject = cwd.join("pyproject.toml");
        if !pyproject.exists() {
            return false;
        }
        fs::read_to_string(pyproject)
            .map(|content| content.contains("name = \"amplihack\""))
            .unwrap_or(false)
    }

    /// Find an active session in the runtime log with a live PID.
    pub fn find_active_session(&self, cwd: &Path) -> Option<SessionEntry> {
        if !self.runtime_log.exists() {
            return None;
        }

        let content = fs::read_to_string(&self.runtime_log).ok()?;
        if content.trim().is_empty() {
            return None;
        }

        // Parse JSONL, track session states
        let mut sessions: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let entry: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let session_id = match entry.get("session_id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            let status = entry
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if status == "completed" || status == "crashed" {
                if let Some(existing) = sessions.get_mut(&session_id) {
                    existing["status"] = serde_json::Value::String(status.to_string());
                }
                continue;
            }

            sessions.insert(session_id, entry);
        }

        let cwd_resolved = cwd.canonicalize().ok()?;

        for data in sessions.values() {
            let status = data
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if status != "active" {
                continue;
            }

            let launch_dir = data
                .get("launch_dir")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let launch_path = Path::new(launch_dir);
            if launch_path.canonicalize().ok().as_deref() != Some(&*cwd_resolved) {
                continue;
            }

            let pid = data.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            if !is_process_alive(pid) {
                continue;
            }

            return Some(SessionEntry {
                pid,
                session_id: data
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                launch_dir: launch_dir.to_string(),
                argv: data
                    .get("argv")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                start_time: data
                    .get("start_time")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                is_auto_mode: data
                    .get("is_auto_mode")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                is_nested: data
                    .get("is_nested")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                parent_session_id: data
                    .get("parent_session_id")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                status: "active".to_string(),
                end_time: data.get("end_time").and_then(|v| v.as_f64()),
            });
        }

        None
    }
}

/// Cross-platform PID liveness check.
fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(unix)]
    {
        // signal 0 checks if process exists without sending a signal
        unsafe {
            let ret = libc::kill(pid as i32, 0);
            if ret == 0 {
                return true;
            }
            // EPERM means process exists but we can't signal it
            *libc::__errno_location() == libc::EPERM
        }
    }

    #[cfg(not(unix))]
    {
        // On non-Unix, assume alive (conservative)
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_amplihack_source_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!NestingDetector::is_amplihack_source_repo(dir.path()));
    }

    #[test]
    fn is_amplihack_source_repo() {
        let dir = tempfile::tempdir().unwrap();
        let pyproject = dir.path().join("pyproject.toml");
        fs::write(
            &pyproject,
            "[project]\nname = \"amplihack\"\nversion = \"1.0\"",
        )
        .unwrap();
        assert!(NestingDetector::is_amplihack_source_repo(dir.path()));
    }

    #[test]
    fn is_not_amplihack_other_project() {
        let dir = tempfile::tempdir().unwrap();
        let pyproject = dir.path().join("pyproject.toml");
        fs::write(&pyproject, "[project]\nname = \"other-project\"").unwrap();
        assert!(!NestingDetector::is_amplihack_source_repo(dir.path()));
    }

    #[test]
    fn detect_nesting_no_log() {
        let dir = tempfile::tempdir().unwrap();
        let detector = NestingDetector::new()
            .with_runtime_log(dir.path().join("nonexistent.jsonl"));
        let result = detector.detect_nesting(dir.path(), &[]);
        assert!(!result.is_nested);
        assert!(!result.requires_staging);
    }

    #[test]
    fn detect_nesting_auto_mode_requires_staging() {
        let dir = tempfile::tempdir().unwrap();
        let detector = NestingDetector::new()
            .with_runtime_log(dir.path().join("nonexistent.jsonl"));
        let result = detector.detect_nesting(dir.path(), &["--auto".to_string()]);
        assert!(result.requires_staging);
    }

    #[test]
    fn find_active_session_empty_log() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("sessions.jsonl");
        fs::write(&log_path, "").unwrap();
        let detector = NestingDetector::new().with_runtime_log(log_path);
        assert!(detector.find_active_session(dir.path()).is_none());
    }

    #[test]
    fn find_active_session_completed() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("sessions.jsonl");
        let launch_dir = dir.path().to_str().unwrap();
        let content = format!(
            "{}\n{}\n",
            serde_json::json!({
                "session_id": "session-abc",
                "pid": 99999,
                "launch_dir": launch_dir,
                "argv": [],
                "start_time": 1000.0,
                "is_auto_mode": false,
                "is_nested": false,
                "status": "active"
            }),
            serde_json::json!({
                "session_id": "session-abc",
                "status": "completed",
                "end_time": 2000.0
            }),
        );
        fs::write(&log_path, content).unwrap();
        let detector = NestingDetector::new().with_runtime_log(log_path);
        assert!(detector.find_active_session(dir.path()).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn process_alive_self() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn process_alive_zero() {
        assert!(!is_process_alive(0));
    }
}
