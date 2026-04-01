//! Nesting detection — prevents accidental recursive amplihack sessions.
//!
//! Detection strategy:
//! 1. Primary: Check `AMPLIHACK_SESSION_ID` env var — if set, we're nested.
//! 2. Secondary: Check `.claude/runtime/sessions.jsonl` in the current project.
//! 3. Fallback: Check session file with PID + timestamp in `~/.claude/runtime/`.
//! 4. Stale detection: If holder PID is dead AND session is >1h old, consider stale.

mod helpers;

use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use helpers::{
    SessionFile, detect_from_sessions_log, is_process_alive, is_stale, session_file_path,
};

/// Result of nesting detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NestingResult {
    /// Not inside another amplihack session.
    NotNested,
    /// Inside an existing amplihack session.
    Nested {
        /// The parent session's ID.
        session_id: String,
        /// Nesting depth (1 = first nested, 2 = doubly nested, etc.).
        depth: u32,
    },
    /// A session file exists but the holder process is dead and session is stale.
    StaleSession {
        /// The stale session's ID.
        session_id: String,
    },
}

/// Detects whether we are inside a nested amplihack session.
pub struct NestingDetector;

impl NestingDetector {
    /// Detect nesting state.
    pub fn detect() -> NestingResult {
        // Primary: Check env var
        if let Ok(session_id) = env::var("AMPLIHACK_SESSION_ID") {
            let depth: u32 = env::var("AMPLIHACK_DEPTH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            return NestingResult::Nested { session_id, depth };
        }

        if let Some(current_dir) = env::current_dir().ok()
            && let Some(result) = detect_from_sessions_log(&current_dir)
        {
            return result;
        }

        // Fallback: Check legacy session file
        if let Some(session_path) = session_file_path()
            && let Ok(content) = std::fs::read_to_string(&session_path)
            && let Ok(session) = serde_json::from_str::<SessionFile>(&content)
        {
            if is_process_alive(session.pid) {
                return NestingResult::Nested {
                    session_id: session.session_id,
                    depth: 1,
                };
            }

            // Process is dead — check staleness
            if is_stale(session.timestamp) {
                return NestingResult::StaleSession {
                    session_id: session.session_id,
                };
            }

            // Process dead but session is recent — assume nested
            // (might have just crashed)
            return NestingResult::Nested {
                session_id: session.session_id,
                depth: 1,
            };
        }

        NestingResult::NotNested
    }

    /// Write a session file to claim the current session.
    pub fn claim_session(session_id: &str) -> anyhow::Result<()> {
        let session_path = session_file_path()
            .ok_or_else(|| anyhow::anyhow!("could not determine HOME directory"))?;

        if let Some(parent) = session_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let session = SessionFile {
            session_id: session_id.to_string(),
            pid: std::process::id(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };

        let content = serde_json::to_string_pretty(&session)?;
        std::fs::write(&session_path, content)?;
        Ok(())
    }

    /// Remove the session file (on clean exit).
    pub fn release_session() {
        if let Some(session_path) = session_file_path() {
            let _ = std::fs::remove_file(session_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        cwd_env_lock, home_env_lock, restore_cwd, restore_home, set_cwd, set_home,
    };
    use helpers::STALE_THRESHOLD;
    use std::fs;

    #[test]
    fn not_nested_when_no_env_var() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Use a temp HOME to isolate from any real session.json on the host.
        let tmp = tempfile::tempdir().unwrap();
        let original_home = set_home(tmp.path());

        // SAFETY: Test-only env var manipulation; test runner serializes tests by default.
        unsafe {
            env::remove_var("AMPLIHACK_SESSION_ID");
            env::remove_var("AMPLIHACK_DEPTH");
        }

        let result = NestingDetector::detect();

        restore_home(original_home);

        assert_eq!(result, NestingResult::NotNested);
    }

    #[test]
    fn nested_when_env_var_set() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // SAFETY: Test-only env var manipulation; test runner serializes tests by default.
        unsafe {
            env::set_var("AMPLIHACK_SESSION_ID", "test-session-123");
            env::set_var("AMPLIHACK_DEPTH", "2");
        }

        let result = NestingDetector::detect();

        // SAFETY: Cleanup after test.
        unsafe {
            env::remove_var("AMPLIHACK_SESSION_ID");
            env::remove_var("AMPLIHACK_DEPTH");
        }

        assert!(matches!(
            result,
            NestingResult::Nested {
                ref session_id,
                depth: 2
            } if session_id == "test-session-123"
        ));
    }

    #[test]
    fn is_stale_old_timestamp() {
        let two_hours_ago = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 7200;
        assert!(helpers::is_stale(two_hours_ago));
    }

    #[test]
    fn is_not_stale_recent_timestamp() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(!helpers::is_stale(now));
    }

    #[cfg(unix)]
    #[test]
    fn current_process_is_alive() {
        assert!(helpers::is_process_alive(std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn dead_process_is_not_alive() {
        // PID 99999999 is almost certainly not alive
        assert!(!helpers::is_process_alive(99_999_999));
    }

    #[test]
    fn claim_and_release_session() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let session_path = dir
            .path()
            .join(".claude")
            .join("runtime")
            .join("session.json");

        let original_home = set_home(dir.path());

        NestingDetector::claim_session("test-claim").unwrap();
        assert!(session_path.exists());

        let content: helpers::SessionFile =
            serde_json::from_str(&std::fs::read_to_string(&session_path).unwrap()).unwrap();
        assert_eq!(content.session_id, "test-claim");
        assert_eq!(content.pid, std::process::id());

        NestingDetector::release_session();
        assert!(!session_path.exists());

        restore_home(original_home);
    }

    #[test]
    fn nested_when_sessions_log_has_active_session_for_current_dir() {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _cwd_guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let project_dir = dir.path().join("project");
        fs::create_dir_all(project_dir.join(".claude/runtime")).unwrap();
        let sessions_log = project_dir.join(".claude/runtime/sessions.jsonl");
        fs::write(
            &sessions_log,
            format!(
                "{{\"pid\":{},\"session_id\":\"session-log-123\",\"launch_dir\":\"{}\",\"argv\":[\"amplihack\",\"claude\"],\"start_time\":0.0,\"is_auto_mode\":false,\"is_nested\":false,\"parent_session_id\":null,\"status\":\"active\",\"end_time\":null}}\n",
                std::process::id(),
                project_dir.display()
            ),
        )
        .unwrap();

        let original_home = set_home(dir.path());
        let original_cwd = set_cwd(&project_dir).unwrap();
        unsafe {
            env::remove_var("AMPLIHACK_SESSION_ID");
            env::remove_var("AMPLIHACK_DEPTH");
        }

        let result = NestingDetector::detect();

        restore_cwd(&original_cwd).unwrap();
        restore_home(original_home);

        assert!(matches!(
            result,
            NestingResult::Nested {
                ref session_id,
                depth: 1
            } if session_id == "session-log-123"
        ));
    }

    #[test]
    fn ignores_completed_session_in_sessions_log() {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _cwd_guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let project_dir = dir.path().join("project");
        fs::create_dir_all(project_dir.join(".claude/runtime")).unwrap();
        let sessions_log = project_dir.join(".claude/runtime/sessions.jsonl");
        fs::write(
            &sessions_log,
            format!(
                "{{\"pid\":{},\"session_id\":\"session-log-123\",\"launch_dir\":\"{}\",\"argv\":[\"amplihack\",\"claude\"],\"start_time\":0.0,\"is_auto_mode\":false,\"is_nested\":false,\"parent_session_id\":null,\"status\":\"active\",\"end_time\":null}}\n{{\"session_id\":\"session-log-123\",\"status\":\"completed\",\"end_time\":1.0}}\n",
                std::process::id(),
                project_dir.display()
            ),
        )
        .unwrap();

        let original_home = set_home(dir.path());
        let original_cwd = set_cwd(&project_dir).unwrap();
        unsafe {
            env::remove_var("AMPLIHACK_SESSION_ID");
            env::remove_var("AMPLIHACK_DEPTH");
        }

        let result = NestingDetector::detect();

        restore_cwd(&original_cwd).unwrap();
        restore_home(original_home);

        assert_eq!(result, NestingResult::NotNested);
    }
}
