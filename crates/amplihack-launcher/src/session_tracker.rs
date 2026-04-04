//! Session tracking for amplihack runtime.
//!
//! Matches Python `amplihack/launcher/session_tracker.py`:
//! - Append-only JSONL session log
//! - Session lifecycle (start → complete/crash)
//! - Unique session IDs

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Represents a single amplihack session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub pid: u32,
    pub session_id: String,
    pub launch_dir: String,
    pub argv: Vec<String>,
    pub start_time: f64,
    pub is_auto_mode: bool,
    pub is_nested: bool,
    pub parent_session_id: Option<String>,
    pub status: String,
    pub end_time: Option<f64>,
}

/// Session status values.
pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_COMPLETED: &str = "completed";
pub const STATUS_CRASHED: &str = "crashed";

/// Manage session lifecycle in `.claude/runtime/sessions.jsonl`.
pub struct SessionTracker {
    runtime_log: PathBuf,
}

impl SessionTracker {
    /// Create a new tracker using the given base directory.
    pub fn new(base_dir: &Path) -> anyhow::Result<Self> {
        let runtime_log = base_dir
            .join(".claude")
            .join("runtime")
            .join("sessions.jsonl");
        let tracker = Self { runtime_log };
        tracker.ensure_runtime_dir()?;
        Ok(tracker)
    }

    fn ensure_runtime_dir(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.runtime_log.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Register a new session, returning its unique ID.
    pub fn start_session(
        &self,
        pid: u32,
        launch_dir: &str,
        argv: &[String],
        is_auto_mode: bool,
        is_nested: bool,
        parent_session_id: Option<&str>,
    ) -> anyhow::Result<String> {
        let session_id = generate_session_id();
        let entry = SessionEntry {
            pid,
            session_id: session_id.clone(),
            launch_dir: launch_dir.to_string(),
            argv: argv.to_vec(),
            start_time: now_secs(),
            is_auto_mode,
            is_nested,
            parent_session_id: parent_session_id.map(String::from),
            status: STATUS_ACTIVE.to_string(),
            end_time: None,
        };
        self.append_entry(&entry)?;
        Ok(session_id)
    }

    /// Mark session as completed.
    pub fn complete_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.end_session(session_id, STATUS_COMPLETED)
    }

    /// Mark session as crashed.
    pub fn crash_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.end_session(session_id, STATUS_CRASHED)
    }

    fn end_session(&self, session_id: &str, status: &str) -> anyhow::Result<()> {
        self.ensure_runtime_dir()?;
        let entry = serde_json::json!({
            "session_id": session_id,
            "status": status,
            "end_time": now_secs(),
        });
        self.write_log(&serde_json::to_string(&entry)?)
    }

    fn append_entry(&self, entry: &SessionEntry) -> anyhow::Result<()> {
        self.ensure_runtime_dir()?;
        self.write_log(&serde_json::to_string(entry)?)
    }

    fn write_log(&self, line: &str) -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.runtime_log)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// Read all entries from the log file.
    pub fn read_entries(&self) -> anyhow::Result<Vec<SessionEntry>> {
        if !self.runtime_log.exists() {
            return Ok(vec![]);
        }
        let content = fs::read_to_string(&self.runtime_log)?;
        let mut entries = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<SessionEntry>(line) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Path to the runtime log.
    pub fn log_path(&self) -> &Path {
        &self.runtime_log
    }
}

fn generate_session_id() -> String {
    // Simple unique ID using timestamp + random suffix
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    format!("session-{ts:x}")
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_tracker() -> (tempfile::TempDir, SessionTracker) {
        let dir = tempfile::tempdir().unwrap();
        let tracker = SessionTracker::new(dir.path()).unwrap();
        (dir, tracker)
    }

    #[test]
    fn start_and_complete_session() {
        let (_dir, tracker) = setup_tracker();
        let sid = tracker
            .start_session(1234, "/home/user/project", &[], false, false, None)
            .unwrap();
        assert!(sid.starts_with("session-"));
        tracker.complete_session(&sid).unwrap();

        let entries = tracker.read_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, STATUS_ACTIVE);
    }

    #[test]
    fn crash_session() {
        let (_dir, tracker) = setup_tracker();
        let sid = tracker
            .start_session(5678, "/project", &["amplihack".into()], true, false, None)
            .unwrap();
        tracker.crash_session(&sid).unwrap();

        // Log should have 2 lines: start + crash
        let content = fs::read_to_string(tracker.log_path()).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        // Second line should have crashed status
        let end_entry: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(end_entry["status"], STATUS_CRASHED);
    }

    #[test]
    fn nested_session() {
        let (_dir, tracker) = setup_tracker();
        let parent_id = tracker
            .start_session(100, "/project", &[], false, false, None)
            .unwrap();
        let child_id = tracker
            .start_session(200, "/project", &[], false, true, Some(&parent_id))
            .unwrap();

        let entries = tracker.read_entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[1].is_nested);
        assert_eq!(
            entries[1].parent_session_id.as_deref(),
            Some(parent_id.as_str())
        );

        tracker.complete_session(&child_id).unwrap();
        tracker.complete_session(&parent_id).unwrap();
    }

    #[test]
    fn auto_mode_flag() {
        let (_dir, tracker) = setup_tracker();
        let sid = tracker
            .start_session(
                42,
                "/project",
                &["amplihack".into(), "--auto".into()],
                true,
                false,
                None,
            )
            .unwrap();

        let entries = tracker.read_entries().unwrap();
        assert!(entries[0].is_auto_mode);
        assert_eq!(entries[0].pid, 42);

        tracker.complete_session(&sid).unwrap();
    }

    #[test]
    fn session_ids_are_unique() {
        let (_dir, tracker) = setup_tracker();
        let s1 = tracker
            .start_session(1, "/a", &[], false, false, None)
            .unwrap();
        let s2 = tracker
            .start_session(2, "/b", &[], false, false, None)
            .unwrap();
        assert_ne!(s1, s2);
    }

    #[test]
    fn read_entries_empty_file() {
        let (_dir, tracker) = setup_tracker();
        let entries = tracker.read_entries().unwrap();
        assert!(entries.is_empty());
    }
}
