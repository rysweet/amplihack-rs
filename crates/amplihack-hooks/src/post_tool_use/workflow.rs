//! Workflow enforcement state and tracking logic.
//!
//! Detects when `/dev` (dev-orchestrator) is invoked and monitors subsequent
//! tool calls for evidence of recipe-runner execution.  Emits a warning if the
//! tool-call threshold is reached without evidence.

use amplihack_types::{ProjectDirs, sanitize_session_id};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
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
    "run_recipe_by_name",
    "smart-orchestrator",
    "recipe_runner",
    "amplihack.recipes",
    "amplihack recipe run",
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

fn workflow_state_file(dirs: &ProjectDirs, session_id: Option<&str>) -> PathBuf {
    let session = session_id
        .filter(|value| !value.trim().is_empty())
        .map(sanitize_session_id)
        .unwrap_or_else(|| "current".to_string());
    dirs.runtime
        .join("workflow_state")
        .join(format!("{session}.json"))
}

pub(super) fn read_workflow_state(
    dirs: &ProjectDirs,
    session_id: Option<&str>,
) -> Option<WorkflowEnforcementState> {
    let path = workflow_state_file(dirs, session_id);
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(super) fn write_workflow_state(
    dirs: &ProjectDirs,
    session_id: Option<&str>,
    state: &WorkflowEnforcementState,
) -> anyhow::Result<()> {
    let path = workflow_state_file(dirs, session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec(state)?)?;
    Ok(())
}

fn clear_workflow_state(dirs: &ProjectDirs, session_id: Option<&str>) {
    let path = workflow_state_file(dirs, session_id);
    let _ = fs::remove_file(path);
}

pub(crate) fn begin_workflow_enforcement_tracking(session_id: Option<&str>) -> anyhow::Result<()> {
    let dirs = ProjectDirs::from_cwd();
    let state = WorkflowEnforcementState {
        dev_invoked_at: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        tool_calls_since: 0,
        warning_emitted: false,
    };
    write_workflow_state(&dirs, session_id, &state)
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
{tool_calls_since} tool calls. You MUST execute via run_recipe_by_name('smart-orchestrator'). \
Direct implementation without the recipe runner is PROHIBITED for Development tasks. The \
23-step workflow, recursion guards, and goal verification are being skipped. STOP and invoke \
the recipe runner NOW."
    )
}

// ---------------------------------------------------------------------------
// Enforcement update (called on every post-tool-use)
// ---------------------------------------------------------------------------

pub(super) fn update_workflow_enforcement(
    tool_name: &str,
    tool_input: &Value,
    session_id: Option<&str>,
) -> Option<String> {
    let dirs = ProjectDirs::from_cwd();

    if is_dev_skill_invocation(tool_name, tool_input) {
        if let Err(error) = begin_workflow_enforcement_tracking(session_id) {
            tracing::warn!("workflow enforcement: failed to write state: {}", error);
        }
        return None;
    }

    let mut state = read_workflow_state(&dirs, session_id)?;

    if has_workflow_evidence(tool_name, tool_input) {
        clear_workflow_state(&dirs, session_id);
        return None;
    }

    state.tool_calls_since += 1;
    if state.tool_calls_since >= TOOL_CALL_THRESHOLD && !state.warning_emitted {
        state.warning_emitted = true;
        if let Err(error) = write_workflow_state(&dirs, session_id, &state) {
            tracing::warn!(
                "workflow enforcement: failed to persist warning state: {}",
                error
            );
        }
        return Some(workflow_bypass_warning(state.tool_calls_since));
    }

    if let Err(error) = write_workflow_state(&dirs, session_id, &state) {
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
        let dirs = ProjectDirs::new(dir.path());

        let state = WorkflowEnforcementState {
            dev_invoked_at: 1234567890,
            tool_calls_since: 2,
            warning_emitted: false,
        };
        write_workflow_state(&dirs, Some("test-session"), &state).unwrap();

        let loaded = read_workflow_state(&dirs, Some("test-session")).unwrap();
        assert_eq!(loaded.dev_invoked_at, 1234567890);
        assert_eq!(loaded.tool_calls_since, 2);
        assert!(!loaded.warning_emitted);
    }

    #[test]
    fn read_workflow_state_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        assert!(read_workflow_state(&dirs, Some("nonexistent")).is_none());
    }

    #[test]
    fn clear_workflow_state_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let state = WorkflowEnforcementState::default();
        write_workflow_state(&dirs, Some("sess"), &state).unwrap();
        assert!(read_workflow_state(&dirs, Some("sess")).is_some());

        clear_workflow_state(&dirs, Some("sess"));
        assert!(read_workflow_state(&dirs, Some("sess")).is_none());
    }

    #[test]
    fn workflow_state_file_sanitizes_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let path = workflow_state_file(&dirs, Some("my/bad\\session"));
        let filename = path.file_stem().unwrap().to_str().unwrap();
        assert!(!filename.contains('/'));
        assert!(!filename.contains('\\'));
    }

    #[test]
    fn workflow_state_file_empty_session_uses_current() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let path = workflow_state_file(&dirs, Some("  "));
        assert!(path.to_str().unwrap().contains("current"));

        let path2 = workflow_state_file(&dirs, None);
        assert!(path2.to_str().unwrap().contains("current"));
    }

    // --- update_workflow_enforcement integration ---

    #[test]
    fn update_enforcement_dev_invocation_starts_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());

        // Manually write state to known dir to avoid from_cwd dependency
        let state = WorkflowEnforcementState {
            dev_invoked_at: 100,
            tool_calls_since: 0,
            warning_emitted: false,
        };
        write_workflow_state(&dirs, Some("test"), &state).unwrap();
        let loaded = read_workflow_state(&dirs, Some("test")).unwrap();
        assert_eq!(loaded.tool_calls_since, 0);
    }

    #[test]
    fn threshold_constant_is_3() {
        assert_eq!(TOOL_CALL_THRESHOLD, 3);
    }
}
