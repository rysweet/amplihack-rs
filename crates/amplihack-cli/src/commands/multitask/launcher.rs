//! Launcher generation and process spawning for workstreams.

use super::models::{ProcessScope, Workstream, WorkstreamScope};
use super::process_scope::{normalize_path, process_start_metadata};
use super::state::persist_state;
use super::utils::{rand_u32, set_executable, tail_output};
use amplihack_types::hook_io::normalize_executable_script_line_endings;
use amplihack_types::workflow;
use anyhow::{Context, Result};
use chrono::Utc;
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

pub(super) fn populate_workstream_scope(ws: &mut Workstream, repo_url: &str, base_ref: &str) {
    let repository = parse_github_repo_identity(repo_url).unwrap_or_else(|| repo_url.to_string());
    let repo_root = normalize_path(&ws.work_dir);
    let tree_id =
        std::env::var("AMPLIHACK_TREE_ID").unwrap_or_else(|_| format!("{:08x}", rand_u32()));
    let recipe_run_id =
        std::env::var("AMPLIHACK_RECIPE_RUN_ID").unwrap_or_else(|_| tree_id.clone());
    let issue_id = ws.issue.to_string();
    ws.workstream_scope = WorkstreamScope {
        repository,
        repo_root: repo_root.clone(),
        workdir: repo_root,
        branch: ws.branch.clone(),
        base_ref: base_ref.to_string(),
        issue_id: issue_id.clone(),
        work_item_id: issue_id,
        recipe: ws.recipe.clone(),
        recipe_run_id,
        tree_id,
        workstream_id: format!("ws-{}", ws.issue),
        expected_title_prefix: ws.description.clone(),
        started_at: Utc::now().to_rfc3339(),
    };
}

fn parse_github_repo_identity(url: &str) -> Option<String> {
    let mut path = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest.to_string()
    } else if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        rest.to_string()
    } else if let Some(rest) = url.strip_prefix("https://github.com/") {
        rest.to_string()
    } else if let Some(rest) = url.strip_prefix("http://github.com/") {
        rest.to_string()
    } else if (url.starts_with("https://") || url.starts_with("http://"))
        && url.contains("@github.com/")
    {
        url.split("@github.com/").nth(1)?.to_string()
    } else {
        return None;
    };
    if let Some((before, _)) = path.split_once('?') {
        path = before.to_string();
    }
    if let Some((before, _)) = path.split_once('#') {
        path = before.to_string();
    }
    path = path.trim_end_matches(".git").to_string();
    let mut parts = path.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() {
        None
    } else {
        Some(format!("{owner}/{repo}"))
    }
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
    write_executable_script(&launcher_sh, &launcher_content)?;
    set_executable(&launcher_sh)?;

    let depth: u32 = std::env::var("AMPLIHACK_SESSION_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let tree_id = if ws.workstream_scope.tree_id.is_empty() {
        std::env::var("AMPLIHACK_TREE_ID").unwrap_or_else(|_| format!("{:08x}", rand_u32()))
    } else {
        ws.workstream_scope.tree_id.clone()
    };
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
        tree_id = tree_id,
        depth = depth + 1,
        max_depth = max_depth,
        max_sessions = max_sessions,
        delegate = delegate,
        issue = ws.issue,
        progress_file = ws.progress_file.display(),
        state_file = ws.state_file.display(),
        worktree_path = ws.worktree_path,
    );
    write_executable_script(&run_sh, &run_content)?;
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
    let tree_id = if ws.workstream_scope.tree_id.is_empty() {
        std::env::var("AMPLIHACK_TREE_ID").unwrap_or_else(|_| format!("{:08x}", rand_u32()))
    } else {
        ws.workstream_scope.tree_id.clone()
    };
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
        tree_id = tree_id,
        depth = depth + 1,
        max_depth = max_depth,
        max_sessions = max_sessions,
        delegate = delegate,
        workflow_selection = workflow::DEFAULT_WORKFLOW_SELECTION,
        dev_orchestrator = workflow::DEV_ORCHESTRATOR_SKILL,
        smart_orchestrator = workflow::SMART_ORCHESTRATOR_RECIPE_COMMAND,
    );
    write_executable_script(&run_sh, &run_content)?;
    set_executable(&run_sh)?;

    Ok(())
}

pub(super) fn write_executable_script(path: &std::path::Path, content: &str) -> Result<()> {
    let normalized = normalize_executable_script_line_endings(content);
    fs::write(path, normalized)
        .with_context(|| format!("write executable launcher script {}", path.display()))
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
    ws.process_scope = ProcessScope {
        pid: Some(child.id()),
        repository: ws.workstream_scope.repository.clone(),
        repo_root: ws.workstream_scope.repo_root.clone(),
        workdir: ws.workstream_scope.workdir.clone(),
        branch: ws.workstream_scope.branch.clone(),
        issue_id: ws.workstream_scope.issue_id.clone(),
        work_item_id: ws.workstream_scope.work_item_id.clone(),
        recipe_run_id: ws.workstream_scope.recipe_run_id.clone(),
        tree_id: ws.workstream_scope.tree_id.clone(),
        workstream_id: ws.workstream_scope.workstream_id.clone(),
        process_started_at: process_start_metadata(child.id())
            .unwrap_or_else(|| Utc::now().to_rfc3339()),
        recorded_at: Utc::now().to_rfc3339(),
    };

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
