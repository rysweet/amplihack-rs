//! Nesting detection — prevents accidental recursive amplihack sessions.
//!
//! Detection strategy:
//! 1. Primary: Check `AMPLIHACK_SESSION_ID` env var — if set, we're nested.
//! 2. Secondary: Check session file with PID + timestamp in `~/.claude/runtime/`.
//! 3. Stale detection: If holder PID is dead AND session is >1h old, consider stale.

use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// Session file format stored in `~/.claude/runtime/session.json`.
#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    session_id: String,
    pid: u32,
    timestamp: u64,
}

const SESSION_FILE_NAME: &str = "session.json";
const STALE_THRESHOLD: Duration = Duration::from_secs(3600); // 1 hour

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

        // Secondary: Check session file
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

fn session_file_path() -> Option<PathBuf> {
    env::var("HOME").ok().map(|h| {
        PathBuf::from(h)
            .join(".claude")
            .join("runtime")
            .join(SESSION_FILE_NAME)
    })
}

/// Check if a process with the given PID is alive.
#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) is a standard POSIX signal-safe way to check if
    // a process exists without actually sending a signal.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    // On non-Unix, assume alive (conservative)
    true
}

fn is_stale(timestamp: u64) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    now.saturating_sub(timestamp) > STALE_THRESHOLD.as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_nested_when_no_env_var() {
        // SAFETY: Test-only env var manipulation; test runner serializes tests by default.
        unsafe {
            env::remove_var("AMPLIHACK_SESSION_ID");
            env::remove_var("AMPLIHACK_DEPTH");
        }

        // Without a session file, should be NotNested
        // (This test may report Nested if there's an actual session file,
        // but in a clean test environment it should be NotNested.)
        let result = NestingDetector::detect();
        // We accept both NotNested and StaleSession in test environments
        assert!(
            matches!(
                result,
                NestingResult::NotNested | NestingResult::StaleSession { .. }
            ),
            "expected NotNested or StaleSession without env var, got: {result:?}"
        );
    }

    #[test]
    fn nested_when_env_var_set() {
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
        assert!(is_stale(two_hours_ago));
    }

    #[test]
    fn is_not_stale_recent_timestamp() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(!is_stale(now));
    }

    #[cfg(unix)]
    #[test]
    fn current_process_is_alive() {
        assert!(is_process_alive(std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn dead_process_is_not_alive() {
        // PID 99999999 is almost certainly not alive
        assert!(!is_process_alive(99_999_999));
    }

    #[test]
    fn claim_and_release_session() {
        let dir = tempfile::tempdir().unwrap();
        let session_path = dir
            .path()
            .join(".claude")
            .join("runtime")
            .join(SESSION_FILE_NAME);

        // Override HOME for this test
        let original_home = env::var("HOME").ok();
        // SAFETY: Test-only env var manipulation; test runner serializes tests by default.
        unsafe { env::set_var("HOME", dir.path()) };

        NestingDetector::claim_session("test-claim").unwrap();
        assert!(session_path.exists());

        let content: SessionFile =
            serde_json::from_str(&std::fs::read_to_string(&session_path).unwrap()).unwrap();
        assert_eq!(content.session_id, "test-claim");
        assert_eq!(content.pid, std::process::id());

        NestingDetector::release_session();
        assert!(!session_path.exists());

        // Restore HOME
        // SAFETY: Restoring original env var.
        if let Some(home) = original_home {
            unsafe { env::set_var("HOME", home) };
        }
    }
}
