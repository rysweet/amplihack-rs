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
