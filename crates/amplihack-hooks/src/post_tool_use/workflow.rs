//! Workflow enforcement state and tracking logic.
//!
//! Detects when `/dev` (dev-orchestrator) is invoked and monitors subsequent
//! tool calls for evidence of recipe-runner execution.  Emits a warning if the
//! tool-call threshold is reached without evidence.
//!
//! The state lives at `<state root>/runtime/sessions/<key>/workflow_state.json`
//! (#1963), where an absent or blank session id uses the key `current` so that
//! an id-less `/dev` prompt and the following tool calls share one file.

use crate::state_dir::create_private_dir_all;
use amplihack_types::{amplihack_state_root, session_state_dir_under};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(super) const TOOL_CALL_THRESHOLD: u64 = 3;

const DEV_SKILL_NAMES: &[&str] = &[
    "dev-orchestrator",
    "amplihack:dev",
    "amplihack:amplihack:dev",
    "default-workflow",
    "amplihack:default-workflow",
    "amplihack:amplihack:default-workflow",
    ".claude:amplihack:dev",
    ".claude:amplihack:default-workflow",
];

const WORKFLOW_EVIDENCE_TOOLS: &[&str] = &["Agent", "agent", "TaskCreate"];

const WORKFLOW_EVIDENCE_BASH: &[&str] = &[
    "amplihack recipe run",
    "smart-orchestrator",
    "recipe-runner-rs",
    "recipe_runner",
    "git checkout -b",
    "git switch -c",
    "git branch ",
    "gh pr create",
    "gh issue create",
];

const WORKFLOW_EVIDENCE_READ: &[&str] = &[
    "DEFAULT_WORKFLOW.md",
    "smart-orchestrator.yaml",
    "default-workflow.yaml",
    "investigation-workflow.yaml",
];

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct WorkflowEnforcementState {
    pub(super) dev_invoked_at: u64,
    pub(super) tool_calls_since: u64,
    pub(super) warning_emitted: bool,
}

/// Session key for workflow state when the hook received no usable id.
const CURRENT_SESSION_KEY: &str = "current";

/// The per-session directory holding the workflow state.
///
/// This is the only place the `current` fallback is applied; the key still
/// goes through `session_state_dir_under`, so it is one sanitised component.
pub(super) fn workflow_session_dir(state_root: &Path, session_id: Option<&str>) -> PathBuf {
    let session = session_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(CURRENT_SESSION_KEY);
    session_state_dir_under(state_root, Some(session))
}

fn workflow_state_file(session_dir: &Path) -> PathBuf {
    session_dir.join("workflow_state.json")
}

pub(super) fn read_workflow_state(session_dir: &Path) -> Option<WorkflowEnforcementState> {
    let raw = fs::read_to_string(workflow_state_file(session_dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(super) fn write_workflow_state(
    session_dir: &Path,
    state: &WorkflowEnforcementState,
) -> anyhow::Result<()> {
    create_private_dir_all(session_dir)?;
    fs::write(workflow_state_file(session_dir), serde_json::to_vec(state)?)?;
    Ok(())
}

fn clear_workflow_state(session_dir: &Path) {
    let path = workflow_state_file(session_dir);
    if let Err(error) = fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            path = %path.display(),
            error = %error,
            "workflow enforcement: failed to clear state"
        );
    }
}

/// Start `/dev` tracking for `session_id` under the amplihack state root.
///
/// This is the only workflow helper that reads the environment. It returns
/// an error when no state root resolves, so callers log and carry on.
pub(crate) fn begin_workflow_enforcement_tracking(session_id: Option<&str>) -> anyhow::Result<()> {
    let state_root = amplihack_state_root()
        .ok_or_else(|| anyhow::anyhow!("no amplihack state root (set HOME or AMPLIHACK_HOME)"))?;
    begin_workflow_enforcement_tracking_in(&workflow_session_dir(&state_root, session_id))
}

/// Write a fresh tracking state into `session_dir`.
pub(super) fn begin_workflow_enforcement_tracking_in(session_dir: &Path) -> anyhow::Result<()> {
    let state = WorkflowEnforcementState {
        dev_invoked_at: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        tool_calls_since: 0,
        warning_emitted: false,
    };
    write_workflow_state(session_dir, &state)
}

fn is_dev_skill_invocation(tool_name: &str, tool_input: &Value) -> bool {
    matches!(tool_name, "Skill" | "skill")
        && tool_input
            .get("skill")
            .and_then(Value::as_str)
            .is_some_and(|skill| DEV_SKILL_NAMES.contains(&skill))
}

fn has_workflow_evidence(tool_name: &str, tool_input: &Value) -> bool {
    if matches!(tool_name, "Bash" | "bash")
        && tool_input
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| {
                WORKFLOW_EVIDENCE_BASH
                    .iter()
                    .any(|pattern| command.contains(pattern))
            })
    {
        return true;
    }

    if matches!(tool_name, "Read" | "read" | "View" | "view")
        && ["file_path", "path"].iter().any(|key| {
            tool_input
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|path| {
                    WORKFLOW_EVIDENCE_READ
                        .iter()
                        .any(|pattern| path.contains(pattern))
                })
        })
    {
        return true;
    }

    WORKFLOW_EVIDENCE_TOOLS.contains(&tool_name)
}

fn workflow_bypass_warning(tool_calls_since: u64) -> String {
    format!(
        "WORKFLOW BYPASS DETECTED: /dev was invoked but no recipe runner execution detected after \
{tool_calls_since} tool calls. You MUST execute via `amplihack recipe run smart-orchestrator`. \
Direct implementation without the recipe runner is PROHIBITED for Development tasks. The \
23-step workflow, recursion guards, and goal verification are being skipped. STOP and invoke \
the recipe runner NOW."
    )
}

// ---------------------------------------------------------------------------
// Enforcement update (called on every post-tool-use)
// ---------------------------------------------------------------------------

pub(super) fn update_workflow_enforcement(
    state_root: &Path,
    tool_name: &str,
    tool_input: &Value,
    session_id: Option<&str>,
) -> Option<String> {
    let session_dir = workflow_session_dir(state_root, session_id);

    if is_dev_skill_invocation(tool_name, tool_input) {
        if let Err(error) = begin_workflow_enforcement_tracking_in(&session_dir) {
            tracing::warn!("workflow enforcement: failed to write state: {}", error);
        }
        return None;
    }

    let mut state = read_workflow_state(&session_dir)?;

    if has_workflow_evidence(tool_name, tool_input) {
        clear_workflow_state(&session_dir);
        return None;
    }

    state.tool_calls_since += 1;
    if state.tool_calls_since >= TOOL_CALL_THRESHOLD && !state.warning_emitted {
        state.warning_emitted = true;
        if let Err(error) = write_workflow_state(&session_dir, &state) {
            tracing::warn!(
                "workflow enforcement: failed to persist warning state: {}",
                error
            );
        }
        return Some(workflow_bypass_warning(state.tool_calls_since));
    }

    if let Err(error) = write_workflow_state(&session_dir, &state) {
        tracing::warn!("workflow enforcement: failed to update state: {}", error);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- is_dev_skill_invocation ---

    #[test]
    fn is_dev_skill_invocation_matches_skill_tool() {
        let input = json!({"skill": "dev-orchestrator"});
        assert!(is_dev_skill_invocation("Skill", &input));
        assert!(is_dev_skill_invocation("skill", &input));
    }

    #[test]
    fn is_dev_skill_invocation_matches_all_aliases() {
        for name in DEV_SKILL_NAMES {
            let input = json!({"skill": name});
            assert!(
                is_dev_skill_invocation("Skill", &input),
                "should match: {name}"
            );
        }
    }

    #[test]
    fn is_dev_skill_invocation_rejects_other_skills() {
        let input = json!({"skill": "merge-ready"});
        assert!(!is_dev_skill_invocation("Skill", &input));
    }

    #[test]
    fn is_dev_skill_invocation_rejects_non_skill_tools() {
        let input = json!({"skill": "dev-orchestrator"});
        assert!(!is_dev_skill_invocation("Bash", &input));
        assert!(!is_dev_skill_invocation("Read", &input));
    }

    #[test]
    fn is_dev_skill_invocation_missing_skill_key() {
        assert!(!is_dev_skill_invocation("Skill", &json!({})));
        assert!(!is_dev_skill_invocation("Skill", &json!({"other": "val"})));
    }

    // --- has_workflow_evidence ---

    #[test]
    fn has_workflow_evidence_bash_recipe_runner() {
        let input = json!({"command": "amplihack recipe run smart-orchestrator"});
        assert!(has_workflow_evidence("Bash", &input));
        assert!(has_workflow_evidence("bash", &input));
    }

    #[test]
    fn has_workflow_evidence_bash_git_checkout() {
        let input = json!({"command": "git checkout -b feat/new-feature"});
        assert!(has_workflow_evidence("Bash", &input));
    }

    #[test]
    fn has_workflow_evidence_bash_no_match() {
        let input = json!({"command": "cargo test"});
        assert!(!has_workflow_evidence("Bash", &input));
    }

    #[test]
    fn has_workflow_evidence_read_workflow_file() {
        let input = json!({"file_path": "/repo/amplifier-bundle/recipes/smart-orchestrator.yaml"});
        assert!(has_workflow_evidence("Read", &input));
    }

    #[test]
    fn has_workflow_evidence_view_path_key() {
        let input = json!({"path": "/home/user/.claude/DEFAULT_WORKFLOW.md"});
        assert!(has_workflow_evidence("View", &input));
        assert!(has_workflow_evidence("view", &input));
    }

    #[test]
    fn has_workflow_evidence_read_no_match() {
        let input = json!({"file_path": "/repo/src/main.rs"});
        assert!(!has_workflow_evidence("Read", &input));
    }

    #[test]
    fn has_workflow_evidence_agent_tool() {
        assert!(has_workflow_evidence("Agent", &json!({})));
        assert!(has_workflow_evidence("agent", &json!({})));
        assert!(has_workflow_evidence("TaskCreate", &json!({})));
    }

    #[test]
    fn has_workflow_evidence_unrelated_tool() {
        assert!(!has_workflow_evidence("Edit", &json!({})));
        assert!(!has_workflow_evidence("grep", &json!({})));
    }

    // --- workflow_bypass_warning ---

    #[test]
    fn workflow_bypass_warning_contains_count() {
        let msg = workflow_bypass_warning(5);
        assert!(msg.contains("5 tool calls"));
        assert!(msg.contains("WORKFLOW BYPASS DETECTED"));
    }

    // --- WorkflowEnforcementState round-trip ---

    #[test]
    fn state_roundtrip_through_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = workflow_session_dir(dir.path(), Some("test-session"));

        let state = WorkflowEnforcementState {
            dev_invoked_at: 1234567890,
            tool_calls_since: 2,
            warning_emitted: false,
        };
        write_workflow_state(&session_dir, &state).unwrap();

        assert!(
            dir.path()
                .join("runtime/sessions/test-session/workflow_state.json")
                .exists()
        );
        let loaded = read_workflow_state(&session_dir).unwrap();
        assert_eq!(loaded.dev_invoked_at, 1234567890);
        assert_eq!(loaded.tool_calls_since, 2);
        assert!(!loaded.warning_emitted);
    }

    #[test]
    fn read_workflow_state_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = workflow_session_dir(dir.path(), Some("nonexistent"));
        assert!(read_workflow_state(&session_dir).is_none());
    }

    #[test]
    fn clear_workflow_state_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = workflow_session_dir(dir.path(), Some("sess"));
        let state = WorkflowEnforcementState::default();
        write_workflow_state(&session_dir, &state).unwrap();
        assert!(read_workflow_state(&session_dir).is_some());

        clear_workflow_state(&session_dir);
        assert!(read_workflow_state(&session_dir).is_none());
    }

    #[test]
    fn workflow_session_dir_sanitizes_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = workflow_session_dir(dir.path(), Some("my/bad\\session"));
        assert_eq!(
            path.parent(),
            Some(dir.path().join("runtime/sessions").as_path())
        );
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(!name.contains('/'));
        assert!(!name.contains('\\'));
    }

    #[test]
    fn workflow_session_dir_empty_session_uses_current() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("runtime/sessions/current");
        assert_eq!(workflow_session_dir(dir.path(), Some("  ")), current);
        assert_eq!(workflow_session_dir(dir.path(), None), current);
    }

    // --- update_workflow_enforcement integration ---

    #[test]
    fn update_enforcement_dev_invocation_starts_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let input = json!({"skill": "dev-orchestrator"});

        assert!(update_workflow_enforcement(dir.path(), "Skill", &input, Some("test")).is_none());

        let session_dir = workflow_session_dir(dir.path(), Some("test"));
        let loaded = read_workflow_state(&session_dir).unwrap();
        assert_eq!(loaded.tool_calls_since, 0);
        assert!(!loaded.warning_emitted);
    }

    #[test]
    fn update_enforcement_warns_at_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let skill = json!({"skill": "dev-orchestrator"});
        let other = json!({"path": "/tmp/x.txt"});
        update_workflow_enforcement(dir.path(), "Skill", &skill, Some("t"));
        for _ in 0..TOOL_CALL_THRESHOLD - 1 {
            assert!(update_workflow_enforcement(dir.path(), "View", &other, Some("t")).is_none());
        }
        let warning = update_workflow_enforcement(dir.path(), "View", &other, Some("t"));
        assert!(warning.unwrap().contains("WORKFLOW BYPASS DETECTED"));
    }

    #[test]
    fn threshold_constant_is_3() {
        assert_eq!(TOOL_CALL_THRESHOLD, 3);
    }
}
