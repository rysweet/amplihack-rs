//! Power steering: checks whether the agent has finished enough work to stop.
//!
//! The checker is intentionally fail-open: if transcript analysis fails, the stop
//! hook approves instead of trapping the user. Within that boundary it now runs
//! native Rust analysis instead of delegating to a stale Python bridge.

mod analysis;
mod decision;
mod state;

use amplihack_state::AtomicCounter;
use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::fs;
use std::path::Path;

#[cfg(test)]
use state::completion_semaphore;

#[derive(Debug, Clone, Copy)]
struct PowerSteeringConfig {
    enabled: bool,
}

impl Default for PowerSteeringConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TranscriptMessage {
    role: String,
    text: String,
    tool_uses: Vec<ToolUse>,
    tool_results: Vec<ToolResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolUse {
    id: Option<String>,
    name: String,
    input: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolResult {
    tool_use_id: Option<String>,
    is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TodoItem {
    label: String,
    status: String,
}

/// Check if power steering should run for this project.
pub fn should_run(dirs: &ProjectDirs) -> bool {
    state::load_config(dirs)
        .map(|config| config.enabled)
        .unwrap_or(false)
}

/// Check power steering state and decide whether to block.
///
/// Returns `Some(block_json)` if the session should be blocked,
/// `None` if it should be approved.
pub fn check(
    dirs: &ProjectDirs,
    session_id: &str,
    transcript_path: Option<&Path>,
) -> anyhow::Result<Option<Value>> {
    let power_steering_dir = dirs.session_power_steering(session_id);
    fs::create_dir_all(&power_steering_dir)?;
    fs::create_dir_all(&dirs.power_steering)?;

    let counter = AtomicCounter::new(power_steering_dir.join("session_count"));
    let count = counter.increment()?;

    // First stop: let the agent end naturally, only enforce on repeated stop
    // attempts after it decided to continue working.
    if count <= 1 {
        return Ok(None);
    }

    if state::is_disabled(dirs) || state::already_completed(dirs, session_id) {
        return Ok(None);
    }

    let Some(path) = transcript_path else {
        tracing::warn!("Power steering transcript missing, approving");
        return Ok(None);
    };

    let messages = match analysis::read_transcript_messages(path) {
        Ok(messages) => messages,
        Err(error) => {
            tracing::warn!("Power steering transcript parsing failed, approving: {error}");
            return Ok(None);
        }
    };

    if messages.is_empty() || analysis::is_qa_session(&messages) {
        return Ok(None);
    }

    let blockers = decision::collect_blockers(&messages, &dirs.root);
    if blockers.is_empty() {
        state::mark_complete(dirs, session_id)?;
        state::write_summary(dirs, session_id, &messages)?;
        return Ok(None);
    }

    Ok(Some(serde_json::json!({
        "decision": "block",
        "reason": decision::build_continuation_prompt(&blockers),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn not_enabled_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        assert!(!should_run(&dirs));
    }

    #[test]
    fn enabled_when_config_exists() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.tools_amplihack).unwrap();
        fs::write(dirs.power_steering_config(), r#"{"enabled": true}"#).unwrap();
        assert!(should_run(&dirs));
    }

    #[test]
    fn first_stop_always_approves() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Implement feature"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Next steps: run tests"}]}}"#,
        );

        let result = check(&dirs, "session-1", Some(&transcript)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn blocks_when_todos_remain_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Finish the migration"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"todo_1","name":"TodoWrite","input":{"todos":[{"content":"Port power steering","status":"in_progress"},{"content":"Run tests","status":"pending"}]}}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done for now."}]}}"#,
        );

        assert!(
            check(&dirs, "session-2", Some(&transcript))
                .unwrap()
                .is_none()
        );
        let result = check(&dirs, "session-2", Some(&transcript))
            .unwrap()
            .unwrap();
        let reason = result.get("reason").and_then(Value::as_str).unwrap();
        assert!(reason.contains("TodoWrite"));
        assert_eq!(result["decision"], "block");
    }

    #[test]
    fn blocks_when_final_response_lists_next_steps() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Continue fixing the Rust port"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"edit_1","name":"Edit","input":{"file_path":"src/lib.rs"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"edit_1","content":"ok","is_error":false}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Implemented the change. Next steps: run cargo test and finish the remaining cleanup."}]}}"#,
        );

        assert!(
            check(&dirs, "session-3", Some(&transcript))
                .unwrap()
                .is_none()
        );
        let result = check(&dirs, "session-3", Some(&transcript))
            .unwrap()
            .unwrap();
        let reason = result.get("reason").and_then(Value::as_str).unwrap();
        assert!(reason.contains("remaining work"));
    }

    #[test]
    fn blocks_when_code_changed_without_tests() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        fs::create_dir_all(dir.path().join("tests")).unwrap();
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Implement the fix"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"edit_1","name":"Edit","input":{"file_path":"src/main.rs"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"edit_1","content":"ok","is_error":false}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Implemented the fix and updated the code."}]}}"#,
        );

        assert!(
            check(&dirs, "session-4", Some(&transcript))
                .unwrap()
                .is_none()
        );
        let result = check(&dirs, "session-4", Some(&transcript))
            .unwrap()
            .unwrap();
        let reason = result.get("reason").and_then(Value::as_str).unwrap();
        assert!(reason.contains("local validation/tests"));
    }

    #[test]
    fn approves_and_marks_complete_after_successful_test_run() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        fs::create_dir_all(dir.path().join("tests")).unwrap();
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Implement the fix"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"edit_1","name":"Edit","input":{"file_path":"src/main.rs"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"edit_1","content":"ok","is_error":false}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"bash_1","name":"Bash","input":{"command":"cargo test -p amplihack-hooks power_steering -- --nocapture"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bash_1","content":"test result: ok. 6 passed; 0 failed","is_error":false}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Implemented the fix, ran cargo test, and all tests passed."}]}}"#,
        );

        assert!(
            check(&dirs, "session-5", Some(&transcript))
                .unwrap()
                .is_none()
        );
        let result = check(&dirs, "session-5", Some(&transcript)).unwrap();
        assert!(result.is_none());
        assert!(completion_semaphore(&dirs, "session-5").exists());
        assert!(
            dirs.session_power_steering("session-5")
                .join("summary.md")
                .exists()
        );
    }

    #[test]
    fn qa_session_skips_power_steering() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = configured_dirs(dir.path());
        let transcript = write_transcript(
            dir.path(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"How do I run the tests?"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Use cargo test from the repo root."}]}}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"What about a single package?"}]}}"#,
        );

        assert!(
            check(&dirs, "session-6", Some(&transcript))
                .unwrap()
                .is_none()
        );
        assert!(
            check(&dirs, "session-6", Some(&transcript))
                .unwrap()
                .is_none()
        );
    }

    fn configured_dirs(root: &Path) -> ProjectDirs {
        let dirs = ProjectDirs::new(root);
        fs::create_dir_all(&dirs.tools_amplihack).unwrap();
        fs::write(dirs.power_steering_config(), r#"{"enabled": true}"#).unwrap();
        dirs
    }

    fn write_transcript(root: &Path, contents: &str) -> PathBuf {
        let path = root.join("transcript.jsonl");
        fs::write(&path, contents).unwrap();
        path
    }
}
