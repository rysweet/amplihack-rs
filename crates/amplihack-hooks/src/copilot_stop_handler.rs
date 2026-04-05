//! Copilot stop handler utilities.
//!
//! Provides continuation-prompt generation, lock-file cleanup, and
//! decision logging for Copilot-driven sessions.

use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;
use tracing::{debug, info, warn};

/// Actions that the Copilot session may recommend on stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopilotAction {
    /// Mark the current goal as complete.
    MarkComplete,
    /// Escalate to the user for guidance.
    Escalate,
    /// Send additional input to continue.
    SendInput,
}

impl CopilotAction {
    /// Parse from a string (case-insensitive).
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "mark_complete" | "markcomplete" | "complete" => Some(Self::MarkComplete),
            "escalate" => Some(Self::Escalate),
            "send_input" | "sendinput" | "input" => Some(Self::SendInput),
            _ => None,
        }
    }
}

/// Generate a continuation prompt based on suggested Copilot actions.
///
/// Returns `Some(prompt)` if the session should continue, or `None` if
/// the goal is considered complete.
pub fn get_copilot_continuation(goal: &str, project_root: &Path) -> Option<String> {
    let suggestions_path = project_root
        .join(".claude")
        .join("runtime")
        .join("copilot-suggestions.json");

    let content = match fs::read_to_string(&suggestions_path) {
        Ok(c) => c,
        Err(e) => {
            debug!("no copilot suggestions file: {e}");
            return None;
        }
    };

    let suggestions: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            warn!("malformed copilot suggestions: {e}");
            return None;
        }
    };

    let action_str = suggestions
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let action = CopilotAction::from_str_loose(action_str);

    match action {
        Some(CopilotAction::MarkComplete) => {
            info!("copilot suggests mark_complete for goal: {goal}");
            None
        }
        Some(CopilotAction::Escalate) => {
            let reason = suggestions
                .get("reasoning")
                .and_then(|v| v.as_str())
                .unwrap_or("needs user guidance");
            Some(format!(
                "Escalating to user: {reason}\n\nOriginal goal: {goal}"
            ))
        }
        Some(CopilotAction::SendInput) => {
            let input = suggestions
                .get("input_text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if input.is_empty() {
                Some(format!("Continue working on: {goal}"))
            } else {
                Some(input.to_string())
            }
        }
        None => {
            debug!("unknown copilot action: {action_str:?}");
            None
        }
    }
}

/// Remove lock-mode files to disable lock mode.
///
/// Cleans up `.lock_active` and `.lock_goal` from the locks directory.
pub fn disable_lock_files(project_root: &Path) {
    let locks_dir = project_root.join(".claude").join("runtime").join("locks");

    let files = [locks_dir.join(".lock_active"), locks_dir.join(".lock_goal")];

    for path in &files {
        match fs::remove_file(path) {
            Ok(()) => info!("removed lock file: {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("failed to remove {}: {e}", path.display()),
        }
    }
}

/// Log a Copilot stop decision as a JSONL line.
///
/// Appends to `.claude/runtime/copilot-decisions/decisions.jsonl` with
/// owner-only file permissions on Unix.
#[allow(clippy::too_many_arguments)]
pub fn log_decision(
    project_root: &Path,
    goal: &str,
    action: &str,
    confidence: f64,
    reasoning: &str,
    input_text: Option<&str>,
    progress_pct: Option<f64>,
) -> Result<()> {
    let decisions_dir = project_root
        .join(".claude")
        .join("runtime")
        .join("copilot-decisions");

    ensure_private_dir(&decisions_dir)?;

    let jsonl_path = decisions_dir.join("decisions.jsonl");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();

    let entry = json!({
        "timestamp": now.as_secs(),
        "goal": goal,
        "action": action,
        "confidence": confidence,
        "reasoning": reasoning,
        "input_text": input_text,
        "progress_pct": progress_pct,
    });

    let line = serde_json::to_string(&entry).context("serialising decision")?;

    let mut file = open_private_append(&jsonl_path)?;
    writeln!(file, "{line}").context("writing decision")?;

    debug!("logged copilot decision: action={action}, confidence={confidence}");
    Ok(())
}

/// Create a directory with 0o700 permissions on Unix.
fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        fs::set_permissions(path, perms)
            .with_context(|| format!("chmod 700 {}", path.display()))?;
    }

    Ok(())
}

/// Open a file for appending with 0o600 permissions on Unix.
fn open_private_append(path: &Path) -> Result<fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))
    }

    #[cfg(not(unix))]
    {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_action_parse() {
        assert_eq!(
            CopilotAction::from_str_loose("mark_complete"),
            Some(CopilotAction::MarkComplete)
        );
        assert_eq!(
            CopilotAction::from_str_loose("ESCALATE"),
            Some(CopilotAction::Escalate)
        );
        assert_eq!(
            CopilotAction::from_str_loose("send_input"),
            Some(CopilotAction::SendInput)
        );
        assert_eq!(CopilotAction::from_str_loose("unknown"), None);
    }

    #[test]
    fn continuation_returns_none_without_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = get_copilot_continuation("build app", dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn continuation_mark_complete() {
        let dir = tempfile::tempdir().unwrap();
        let suggestions_dir = dir.path().join(".claude").join("runtime");
        fs::create_dir_all(&suggestions_dir).unwrap();
        fs::write(
            suggestions_dir.join("copilot-suggestions.json"),
            r#"{"action": "mark_complete"}"#,
        )
        .unwrap();

        let result = get_copilot_continuation("build app", dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn continuation_send_input() {
        let dir = tempfile::tempdir().unwrap();
        let suggestions_dir = dir.path().join(".claude").join("runtime");
        fs::create_dir_all(&suggestions_dir).unwrap();
        fs::write(
            suggestions_dir.join("copilot-suggestions.json"),
            r#"{"action": "send_input", "input_text": "fix the tests"}"#,
        )
        .unwrap();

        let result = get_copilot_continuation("build app", dir.path());
        assert_eq!(result, Some("fix the tests".to_string()));
    }

    #[test]
    fn continuation_escalate() {
        let dir = tempfile::tempdir().unwrap();
        let suggestions_dir = dir.path().join(".claude").join("runtime");
        fs::create_dir_all(&suggestions_dir).unwrap();
        fs::write(
            suggestions_dir.join("copilot-suggestions.json"),
            r#"{"action": "escalate", "reasoning": "stuck on auth"}"#,
        )
        .unwrap();

        let result = get_copilot_continuation("build app", dir.path());
        assert!(result.is_some());
        let prompt = result.unwrap();
        assert!(prompt.contains("stuck on auth"));
        assert!(prompt.contains("build app"));
    }

    #[test]
    fn disable_lock_files_removes_locks() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join(".claude").join("runtime").join("locks");
        fs::create_dir_all(&locks_dir).unwrap();
        fs::write(locks_dir.join(".lock_active"), "").unwrap();
        fs::write(locks_dir.join(".lock_goal"), "fix bug").unwrap();

        disable_lock_files(dir.path());

        assert!(!locks_dir.join(".lock_active").exists());
        assert!(!locks_dir.join(".lock_goal").exists());
    }

    #[test]
    fn disable_lock_files_tolerates_missing() {
        let dir = tempfile::tempdir().unwrap();
        // No lock files — should not panic.
        disable_lock_files(dir.path());
    }

    #[test]
    fn log_decision_creates_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        log_decision(
            dir.path(),
            "deploy",
            "send_input",
            0.85,
            "tests passing",
            Some("run deploy"),
            Some(75.0),
        )
        .unwrap();

        let jsonl_path = dir
            .path()
            .join(".claude")
            .join("runtime")
            .join("copilot-decisions")
            .join("decisions.jsonl");
        assert!(jsonl_path.exists());

        let content = fs::read_to_string(&jsonl_path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry["goal"], "deploy");
        assert_eq!(entry["action"], "send_input");
        assert_eq!(entry["confidence"], 0.85);
    }

    #[test]
    fn log_decision_appends() {
        let dir = tempfile::tempdir().unwrap();
        log_decision(dir.path(), "g1", "a1", 0.5, "r1", None, None).unwrap();
        log_decision(dir.path(), "g2", "a2", 0.9, "r2", None, None).unwrap();

        let jsonl_path = dir
            .path()
            .join(".claude")
            .join("runtime")
            .join("copilot-decisions")
            .join("decisions.jsonl");
        let content = fs::read_to_string(&jsonl_path).unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);
    }
}
