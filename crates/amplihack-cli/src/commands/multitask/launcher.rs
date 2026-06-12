//! Launcher generation and process spawning for workstreams.

use super::models::Workstream;
use super::state::persist_state;
use super::utils::{rand_u32, set_executable, tail_output};
use amplihack_types::workflow;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;
use tracing::warn;

/// Valid delegate commands for subprocess execution.
pub(super) const VALID_DELEGATES: &[&str] = &[
    "amplihack claude",
    "amplihack copilot",
    "amplihack amplifier",
];

/// Detect which delegate command to use from the environment.
pub(super) fn detect_delegate() -> String {
    if let Ok(delegate) = std::env::var("AMPLIHACK_DELEGATE") {
        if VALID_DELEGATES.contains(&delegate.as_str()) {
            return delegate;
        }
        warn!("AMPLIHACK_DELEGATE={delegate:?} is not valid. Using default.");
    }
    // Default to claude
    "amplihack claude".to_string()
}

/// Build context map for recipe-based resume.
pub(super) fn build_resume_context(ws: &Workstream) -> HashMap<String, serde_json::Value> {
    let mut ctx = HashMap::new();
    ctx.insert(
        "task_description".to_string(),
        serde_json::Value::String(ws.task.clone()),
    );
    ctx.insert(
        "repo_path".to_string(),
        serde_json::Value::String(".".to_string()),
    );
    ctx.insert("issue_number".to_string(), serde_json::json!(ws.issue));
    ctx.insert(
        "workstream_state_file".to_string(),
        serde_json::Value::String(ws.state_file.to_string_lossy().to_string()),
    );
    ctx.insert(
        "workstream_progress_file".to_string(),
        serde_json::Value::String(ws.progress_file.to_string_lossy().to_string()),
    );
    if !ws.resume_checkpoint.is_empty() {
        ctx.insert(
            "resume_checkpoint".to_string(),
            serde_json::Value::String(ws.resume_checkpoint.clone()),
        );
    }
    if !ws.worktree_path.is_empty() {
        ctx.insert(
            "worktree_setup".to_string(),
            serde_json::json!({
                "worktree_path": ws.worktree_path,
                "branch_name": ws.branch,
                "created": false,
            }),
        );
    }
    ctx
}

/// Write recipe-mode launcher scripts for a workstream.
pub(super) fn write_recipe_launcher(ws: &Workstream, delegate: &str) -> Result<()> {
    let resume_context = build_resume_context(ws);
    let safe_recipe = &ws.recipe;
    let safe_context = serde_json::to_string(&resume_context)?;

    // Write context JSON so the launcher script can pass it via -c flags
    let context_json = ws.work_dir.join("context.json");
    fs::write(&context_json, &safe_context)?;

    let launcher_sh = ws.work_dir.join("launcher.sh");
    let launcher_content = format!(
        r#"#!/bin/bash
# Workstream launcher - Rust recipe runner execution.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
CONTEXT_JSON="$REPO_ROOT/context.json"

# Build -c flags from context JSON
CONTEXT_FLAGS=""
if command -v jq >/dev/null 2>&1 && [ -f "$CONTEXT_JSON" ]; then
    while IFS='=' read -r key value; do
        CONTEXT_FLAGS="$CONTEXT_FLAGS -c $key=$value"
    done < <(jq -r 'to_entries[] | "\(.key)=\(.value)"' "$CONTEXT_JSON")
fi

echo "Starting recipe: {recipe}"
echo "Work dir: $REPO_ROOT"

exec amplihack recipe run {recipe} $CONTEXT_FLAGS --verbose
"#,
        recipe = safe_recipe,
    );
    fs::write(&launcher_sh, launcher_content)?;
    set_executable(&launcher_sh)?;

    let depth: u32 = std::env::var("AMPLIHACK_SESSION_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let tree_id =
        std::env::var("AMPLIHACK_TREE_ID").unwrap_or_else(|_| format!("{:08x}", rand_u32()));
    let max_depth = std::env::var("AMPLIHACK_MAX_DEPTH").unwrap_or_else(|_| "3".to_string());
    let max_sessions = std::env::var("AMPLIHACK_MAX_SESSIONS").unwrap_or_else(|_| "10".to_string());

    let run_sh = ws.work_dir.join("run.sh");
    let run_content = format!(
        r#"#!/bin/bash
cd '{work_dir}'
export AMPLIHACK_TREE_ID='{tree_id}'
export AMPLIHACK_SESSION_DEPTH='{depth}'
export AMPLIHACK_MAX_DEPTH='{max_depth}'
export AMPLIHACK_MAX_SESSIONS='{max_sessions}'
export AMPLIHACK_DELEGATE='{delegate}'
export AMPLIHACK_WORKSTREAM_ISSUE='{issue}'
export AMPLIHACK_WORKSTREAM_PROGRESS_FILE='{progress_file}'
export AMPLIHACK_WORKSTREAM_STATE_FILE='{state_file}'
export AMPLIHACK_WORKTREE_PATH='{worktree_path}'
exec bash launcher.sh
"#,
        work_dir = ws.work_dir.display(),
        depth = depth + 1,
        issue = ws.issue,
        progress_file = ws.progress_file.display(),
        state_file = ws.state_file.display(),
        worktree_path = ws.worktree_path,
    );
    fs::write(&run_sh, run_content)?;
    set_executable(&run_sh)?;

    Ok(())
}

/// Write classic-mode launcher scripts for a workstream.
pub(super) fn write_classic_launcher(ws: &Workstream, delegate: &str) -> Result<()> {
    let task_md = ws.work_dir.join("TASK.md");
    fs::write(
        &task_md,
        format!(
            "# Issue #{}\n\n{}\n\nUse the canonical {} autonomously via {} and {}. \
             NO QUESTIONS. Work through all required workflow steps. Create PR when complete.",
            ws.issue,
            ws.task,
            workflow::DEFAULT_WORKFLOW_SELECTION,
            workflow::DEV_ORCHESTRATOR_SKILL,
            workflow::SMART_ORCHESTRATOR_RECIPE_COMMAND
        ),
    )?;

    let depth: u32 = std::env::var("AMPLIHACK_SESSION_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let tree_id =
        std::env::var("AMPLIHACK_TREE_ID").unwrap_or_else(|_| format!("{:08x}", rand_u32()));
    let max_depth = std::env::var("AMPLIHACK_MAX_DEPTH").unwrap_or_else(|_| "3".to_string());
    let max_sessions = std::env::var("AMPLIHACK_MAX_SESSIONS").unwrap_or_else(|_| "10".to_string());

    let run_sh = ws.work_dir.join("run.sh");
    let run_content = format!(
        r#"#!/bin/bash
cd '{work_dir}'
export AMPLIHACK_TREE_ID='{tree_id}'
export AMPLIHACK_SESSION_DEPTH='{depth}'
export AMPLIHACK_MAX_DEPTH='{max_depth}'
export AMPLIHACK_MAX_SESSIONS='{max_sessions}'
{delegate} --subprocess-safe -- -p "@TASK.md Execute task autonomously using the canonical {workflow_selection} via {dev_orchestrator} and {smart_orchestrator}. NO QUESTIONS. Work through all required workflow steps. Create PR when complete."
"#,
        work_dir = ws.work_dir.display(),
        depth = depth + 1,
        workflow_selection = workflow::DEFAULT_WORKFLOW_SELECTION,
        dev_orchestrator = workflow::DEV_ORCHESTRATOR_SKILL,
        smart_orchestrator = workflow::SMART_ORCHESTRATOR_RECIPE_COMMAND,
    );
    fs::write(&run_sh, run_content)?;
    set_executable(&run_sh)?;

    Ok(())
}

/// Launch a single workstream subprocess.
pub(super) fn launch_workstream(
    ws: &mut Workstream,
    mode: &str,
    delegate: &str,
    processes: &mut HashMap<i64, Arc<Mutex<Option<Child>>>>,
) -> Result<()> {
    let issue = ws.issue;

    let mut env_vars: HashMap<String, String> = std::env::vars().collect();
    env_vars.insert("AMPLIHACK_DELEGATE".to_string(), delegate.to_string());

    let run_sh = ws.work_dir.join("run.sh");
    let child = Command::new("bash")
        .arg(&run_sh)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(&ws.work_dir)
        .envs(&env_vars)
        .spawn()
        .with_context(|| format!("[{issue}] Failed to launch workstream"))?;

    ws.pid = Some(child.id());
    ws.start_time = Some(Instant::now());
    ws.end_time = None;
    ws.exit_code = None;
    ws.lifecycle_state = "running".to_string();
    ws.cleanup_eligible = false;
    ws.attempt += 1;

    println!("[{issue}] Launched PID {} ({mode} mode)", child.id());

    let child_arc = Arc::new(Mutex::new(Some(child)));
    processes.insert(issue, child_arc.clone());

    // Spawn output tailing thread
    let log_file = ws.log_file.clone();
    let max_log_bytes: u64 = std::env::var("AMPLIHACK_MAX_LOG_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100 * 1024 * 1024);

    thread::spawn(move || {
        let mut child_guard = child_arc.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref mut child) = *child_guard
            && let Some(stdout) = child.stdout.take()
        {
            drop(child_guard);
            tail_output(stdout, &log_file, issue, max_log_bytes);
        }
    });

    persist_state(ws)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_delegates() {
        assert!(VALID_DELEGATES.contains(&"amplihack claude"));
        assert!(VALID_DELEGATES.contains(&"amplihack copilot"));
        assert!(VALID_DELEGATES.contains(&"amplihack amplifier"));
        assert!(!VALID_DELEGATES.contains(&"rm -rf /"));
    }
}
