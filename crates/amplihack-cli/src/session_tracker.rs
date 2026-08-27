//! Launcher session lifecycle tracking.

use crate::nesting::NestingResult;
use crate::session_log;
use amplihack_types::ProjectDirs;
use anyhow::{Context, Result};
use fs4::fs_std::FileExt;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Suffix of the advisory lock guarding append + compaction of the session
/// log (issue #1272).
///
/// The lock lives in a *sibling* file rather than the log itself because
/// compaction replaces the log's inode via `rename`: a lock held on the log
/// would be a lock on the inode that is about to be discarded. Appends are
/// cheap enough that the extra `open`/`flock` pair is not measurable next to
/// the JSON encoding they guard.
const LOCK_FILE_SUFFIX: &str = ".lock";

/// Exclusive advisory lock over the session log, released on drop.
struct LogLock {
    file: File,
}

impl LogLock {
    fn acquire(log_path: &Path) -> Result<Self> {
        let mut name = log_path.as_os_str().to_os_string();
        name.push(LOCK_FILE_SUFFIX);
        let lock_path = PathBuf::from(name);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open {}", lock_path.display()))?;
        FileExt::lock_exclusive(&file)
            .with_context(|| format!("failed to lock {}", lock_path.display()))?;
        restrict_permissions(&lock_path);
        Ok(Self { file })
    }
}

impl Drop for LogLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

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
        // Issue #97: tolerate missing parent dir — the cosmetic ENOENT at
        // shutdown was because `create(true)` does not create intermediate
        // directories. Ensure the runtime dir exists before opening.
        if let Some(parent) = self.log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // Issue #1272: the append and the retention check share one advisory
        // lock. Without it a concurrent launch could open the log, have
        // compaction rename a new inode into place underneath it, and then
        // write its record into the orphaned old inode — losing an entry.
        let _lock = LogLock::acquire(&self.log_path)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .with_context(|| format!("failed to open {}", self.log_path.display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("failed to write {}", self.log_path.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("failed to write newline to {}", self.log_path.display()))?;
        drop(file);
        restrict_permissions(&self.log_path);
        // Bound the log so the launch-path reader's cost cannot grow without
        // limit. A compaction failure must not fail the session it is only
        // tidying up after, so it is reported and swallowed here.
        if let Err(err) = session_log::compact_if_needed(&self.log_path) {
            tracing::warn!(
                "session log compaction failed for {}: {err:#}",
                self.log_path.display()
            );
        }
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
    fn append_line_creates_missing_runtime_dir() {
        // Issue #97: session tracker must auto-create the parent runtime
        // directory. Previously `OpenOptions::create(true)` only created the
        // file, not intermediate dirs, so ending a session before any prior
        // write produced a cosmetic ENOENT on shutdown that surfaced as
        // "Status: ✗ Failed" in recipe-runner consumers.
        let temp = tempfile::tempdir().unwrap();
        let tracker = SessionTracker::new(temp.path()).unwrap();
        // Simulate the race: runtime dir removed between tracker construction
        // and append (e.g. by a sibling cleanup hook).
        let runtime = tracker.log_path.parent().unwrap().to_path_buf();
        assert!(runtime.exists());
        std::fs::remove_dir_all(&runtime).unwrap();
        assert!(!runtime.exists());
        tracker
            .complete_session("no-prior-session")
            .expect("complete_session must tolerate missing runtime dir");
        assert!(runtime.join("sessions.jsonl").exists());
    }

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

    /// Issue #1272: appending to an already-huge log must bound it, and the
    /// bounding must not cost a session that is still running.
    #[test]
    fn appending_to_an_oversized_log_bounds_it_without_losing_a_live_session() {
        let dir = tempfile::tempdir().unwrap();
        let tracker = SessionTracker::new(dir.path()).unwrap();
        let log = dir.path().join(".claude/runtime/sessions.jsonl");

        // A live session whose record sits at the very top of the log.
        let argv = vec!["amplihack".to_string(), "claude".to_string()];
        let live = tracker
            .start_session(
                std::process::id(),
                dir.path(),
                &argv,
                false,
                &NestingResult::NotNested,
            )
            .unwrap();

        // Bury it under megabytes of long-finished history, exactly the shape
        // real logs take (fat `argv` prompts, matched start/end pairs).
        let mut history = fs::read_to_string(&log).unwrap();
        for index in 0..4_000u32 {
            let session_id = format!("session-old-{index:08x}");
            history.push_str(
                &serde_json::json!({
                    "pid": 900_000 + index,
                    "session_id": session_id,
                    "launch_dir": dir.path().display().to_string(),
                    "argv": ["amplihack", "claude", "-p", "y".repeat(512)],
                    "start_time": 1_000.0,
                    "is_auto_mode": false,
                    "is_nested": false,
                    "parent_session_id": Value::Null,
                    "status": "active",
                    "end_time": Value::Null,
                })
                .to_string(),
            );
            history.push('\n');
            history.push_str(
                &serde_json::json!({
                    "session_id": session_id,
                    "status": "completed",
                    "end_time": 1_001.0,
                })
                .to_string(),
            );
            history.push('\n');
        }
        fs::write(&log, &history).unwrap();
        let before = fs::metadata(&log).unwrap().len();
        assert!(before > crate::session_log::MAX_LOG_BYTES);

        // The next launch's append triggers compaction.
        let next = tracker
            .start_session(
                std::process::id(),
                dir.path(),
                &argv,
                false,
                &NestingResult::NotNested,
            )
            .unwrap();

        let after = fs::metadata(&log).unwrap().len();
        assert!(
            after < before,
            "an oversized log must be bounded on append ({before} -> {after})"
        );
        let content = fs::read_to_string(&log).unwrap();
        assert!(
            content.contains(&live),
            "the still-running session must survive compaction"
        );
        assert!(
            content.contains(&next),
            "the record that triggered compaction must survive it"
        );
        for line in content.lines() {
            serde_json::from_str::<Value>(line).expect("log must stay valid JSONL");
        }
    }

    /// Issue #1272 acceptance: concurrent launches appending while a
    /// compaction is in flight must neither corrupt the file nor lose entries.
    #[test]
    fn concurrent_appends_across_a_compaction_lose_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let tracker = SessionTracker::new(dir.path()).unwrap();
        let log = dir.path().join(".claude/runtime/sessions.jsonl");

        // Seed the log just over the compaction threshold so the very first
        // concurrent append trips a rewrite while the others are mid-flight.
        let mut history = String::new();
        for index in 0..3_000u32 {
            let session_id = format!("session-old-{index:08x}");
            history.push_str(
                &serde_json::json!({
                    "pid": 900_000 + index,
                    "session_id": session_id,
                    "launch_dir": dir.path().display().to_string(),
                    "argv": ["amplihack", "claude", "-p", "y".repeat(512)],
                    "start_time": 1_000.0,
                    "is_auto_mode": false,
                    "is_nested": false,
                    "parent_session_id": Value::Null,
                    "status": "active",
                    "end_time": Value::Null,
                })
                .to_string(),
            );
            history.push('\n');
            history.push_str(
                &serde_json::json!({
                    "session_id": session_id,
                    "status": "completed",
                    "end_time": 1_001.0,
                })
                .to_string(),
            );
            history.push('\n');
        }
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        fs::write(&log, &history).unwrap();
        assert!(fs::metadata(&log).unwrap().len() > crate::session_log::MAX_LOG_BYTES);

        // Every appended session stays `active` with this process's live PID,
        // so retention may never drop it: anything missing at the end was lost.
        let started = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let tracker = tracker.clone();
                let root = dir.path().to_path_buf();
                let started = std::sync::Arc::clone(&started);
                scope.spawn(move || {
                    let argv = vec!["amplihack".to_string(), "claude".to_string()];
                    for _ in 0..10 {
                        let id = tracker
                            .start_session(
                                std::process::id(),
                                &root,
                                &argv,
                                false,
                                &NestingResult::NotNested,
                            )
                            .expect("append must succeed under contention");
                        started.lock().unwrap().push(id);
                    }
                });
            }
        });

        let content = fs::read_to_string(&log).unwrap();
        for line in content.lines() {
            serde_json::from_str::<Value>(line).expect("concurrent appends must not tear lines");
        }
        let started = started.lock().unwrap();
        // Session ids are derived from a nanosecond clock; identical ids from
        // two threads in the same nanosecond would make this test lie, so
        // check the set we actually recorded.
        let unique: std::collections::BTreeSet<&String> = started.iter().collect();
        for session_id in &unique {
            assert!(
                content.contains(session_id.as_str()),
                "record for still-active session {session_id} was lost across a compaction"
            );
        }
        assert!(
            unique.len() >= 70,
            "fixture should have produced ~80 distinct sessions, got {}",
            unique.len()
        );
    }
}
