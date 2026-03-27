//! Launcher session lifecycle tracking.

use crate::nesting::NestingResult;
use amplihack_types::ProjectDirs;
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize)]
struct SessionStartEntry<'a> {
    pid: u32,
    session_id: &'a str,
    launch_dir: String,
    argv: &'a [String],
    start_time: f64,
    is_auto_mode: bool,
    is_nested: bool,
    parent_session_id: Option<String>,
    status: &'static str,
    end_time: Option<f64>,
}

#[derive(Debug, Serialize)]
struct SessionEndEntry<'a> {
    session_id: &'a str,
    status: &'static str,
    end_time: f64,
}

#[derive(Debug, Clone)]
pub struct SessionTracker {
    log_path: PathBuf,
}

impl SessionTracker {
    pub fn new(project_root: &Path) -> Result<Self> {
        let dirs = ProjectDirs::from_root(project_root);
        fs::create_dir_all(&dirs.runtime)
            .with_context(|| format!("failed to create {}", dirs.runtime.display()))?;
        Ok(Self {
            log_path: dirs.sessions_log_file(),
        })
    }

    pub fn start_session(
        &self,
        pid: u32,
        launch_dir: &Path,
        argv: &[String],
        is_auto_mode: bool,
        nesting: &NestingResult,
    ) -> Result<String> {
        let session_id = generate_session_id();
        let entry = SessionStartEntry {
            pid,
            session_id: &session_id,
            launch_dir: launch_dir.display().to_string(),
            argv,
            start_time: now_secs_f64(),
            is_auto_mode,
            is_nested: matches!(nesting, NestingResult::Nested { .. }),
            parent_session_id: match nesting {
                NestingResult::Nested { session_id, .. } => Some(session_id.clone()),
                _ => None,
            },
            status: "active",
            end_time: None,
        };
        self.append_line(&entry)?;
        Ok(session_id)
    }

    pub fn complete_session(&self, session_id: &str) -> Result<()> {
        self.finish_session(session_id, "completed")
    }

    pub fn crash_session(&self, session_id: &str) -> Result<()> {
        self.finish_session(session_id, "crashed")
    }

    fn finish_session(&self, session_id: &str, status: &'static str) -> Result<()> {
        let entry = SessionEndEntry {
            session_id,
            status,
            end_time: now_secs_f64(),
        };
        self.append_line(&entry)
    }

    fn append_line<T: Serialize>(&self, entry: &T) -> Result<()> {
        let line = serde_json::to_string(entry).context("failed to encode session entry")?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .with_context(|| format!("failed to open {}", self.log_path.display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("failed to write {}", self.log_path.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("failed to write newline to {}", self.log_path.display()))?;
        restrict_permissions(&self.log_path);
        Ok(())
    }
}

fn now_secs_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

fn generate_session_id() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "session-{:08x}",
        (stamp ^ std::process::id() as u128) as u64
    )
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        let _ = fs::set_permissions(path, permissions);
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn session_tracker_writes_start_and_complete_entries() {
        let dir = tempfile::tempdir().unwrap();
        let tracker = SessionTracker::new(dir.path()).unwrap();
        let argv = vec!["amplihack".to_string(), "claude".to_string()];

        let session_id = tracker
            .start_session(42, dir.path(), &argv, false, &NestingResult::NotNested)
            .unwrap();
        tracker.complete_session(&session_id).unwrap();

        let content =
            fs::read_to_string(dir.path().join(".claude/runtime/sessions.jsonl")).unwrap();
        let entries = content
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["status"], "active");
        assert_eq!(entries[0]["session_id"], session_id);
        assert_eq!(entries[0]["argv"][0], "amplihack");
        assert_eq!(entries[1]["status"], "completed");
        assert_eq!(entries[1]["session_id"], session_id);
    }

    #[test]
    fn session_tracker_records_parent_session_for_nested_runs() {
        let dir = tempfile::tempdir().unwrap();
        let tracker = SessionTracker::new(dir.path()).unwrap();
        let argv = vec!["amplihack".to_string(), "copilot".to_string()];

        let session_id = tracker
            .start_session(
                7,
                dir.path(),
                &argv,
                true,
                &NestingResult::Nested {
                    session_id: "parent-123".to_string(),
                    depth: 2,
                },
            )
            .unwrap();

        let content =
            fs::read_to_string(dir.path().join(".claude/runtime/sessions.jsonl")).unwrap();
        let entry = serde_json::from_str::<Value>(content.lines().next().unwrap()).unwrap();
        assert_eq!(entry["session_id"], session_id);
        assert_eq!(entry["is_auto_mode"], true);
        assert_eq!(entry["is_nested"], true);
        assert_eq!(entry["parent_session_id"], "parent-123");
    }
}
