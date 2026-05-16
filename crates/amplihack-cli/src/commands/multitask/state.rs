//! State persistence and lifecycle management for workstreams.

use super::models::*;
use super::utils::{atomic_write, dir_size_bytes};
use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Status snapshot for all workstreams.
#[derive(Default)]
pub(super) struct WorkstreamStatus {
    pub running: Vec<i64>,
    pub completed: Vec<i64>,
    pub failed: Vec<i64>,
}

/// Load persisted state from a JSON file.
pub(super) fn load_state(state_file: &Path) -> Option<PersistedState> {
    fs::read_to_string(state_file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

/// Persist workstream state to its JSON state file.
pub(super) fn persist_state(ws: &Workstream) -> Result<()> {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let existing = load_state(&ws.state_file);
    let created_at = existing
        .as_ref()
        .and_then(|s| {
            if s.created_at.is_empty() {
                None
            } else {
                Some(s.created_at.clone())
            }
        })
        .unwrap_or_else(|| now.clone());

    let state = PersistedState {
        issue: ws.issue,
        branch: ws.branch.clone(),
        description: ws.description.clone(),
        task: ws.task.clone(),
        recipe: ws.recipe.clone(),
        lifecycle_state: ws.lifecycle_state.clone(),
        cleanup_eligible: ws.cleanup_eligible,
        attempt: ws.attempt,
        last_pid: ws.pid,
        last_exit_code: ws.exit_code,
        current_step: if ws.last_step.is_empty() {
            "unknown".to_string()
        } else {
            ws.last_step.clone()
        },
        checkpoint_id: ws.checkpoint_id.clone(),
        work_dir: ws.work_dir.to_string_lossy().to_string(),
        worktree_path: ws.worktree_path.clone(),
        log_file: ws.log_file.to_string_lossy().to_string(),
        progress_sidecar: ws.progress_file.to_string_lossy().to_string(),
        max_runtime: Some(ws.max_runtime),
        timeout_policy: Some(ws.timeout_policy.clone()),
        created_at,
        updated_at: now,
        resume_context: existing.and_then(|s| s.resume_context),
    };

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write(&ws.state_file, json.as_bytes())?;
    Ok(())
}

/// Finalize a workstream after its process exits.
pub(super) fn finalize_workstream(ws: &mut Workstream, exit_code: i32) {
    if ws.end_time.is_none() {
        ws.end_time = Some(Instant::now());
    }
    ws.exit_code = Some(exit_code);

    if ws.lifecycle_state == "interrupted_resumable"
        || (ws.lifecycle_state == "timed_out_resumable"
            && ws.timeout_policy == INTERRUPT_PRESERVE_TIMEOUT_POLICY)
    {
        let _ = persist_state(ws);
        return;
    }

    if exit_code == 0 {
        ws.lifecycle_state = "completed".to_string();
    } else if !ws.checkpoint_id.is_empty() {
        ws.lifecycle_state = "failed_resumable".to_string();
    } else {
        ws.lifecycle_state = "failed_terminal".to_string();
    }
    ws.cleanup_eligible = Workstream::derive_cleanup_eligible(&ws.lifecycle_state);
    let _ = persist_state(ws);
}

/// Apply saved state to a workstream being resumed.
pub(super) fn apply_saved_state(ws: &mut Workstream, state: &PersistedState) {
    if !state.branch.is_empty() {
        ws.branch = state.branch.clone();
    }
    if !state.description.is_empty() {
        ws.description = state.description.clone();
    }
    if !state.lifecycle_state.is_empty() {
        ws.lifecycle_state = state.lifecycle_state.clone();
    }
    ws.cleanup_eligible =
        state.cleanup_eligible || Workstream::derive_cleanup_eligible(&ws.lifecycle_state);
    if !state.worktree_path.is_empty() {
        ws.worktree_path = state.worktree_path.clone();
    }
    if !state.checkpoint_id.is_empty() {
        ws.checkpoint_id = state.checkpoint_id.clone();
        ws.resume_checkpoint = state.checkpoint_id.clone();
    }
    if !state.current_step.is_empty() {
        ws.last_step = state.current_step.clone();
    }
    ws.attempt = state.attempt;
    if let Some(max_rt) = state.max_runtime {
        ws.max_runtime = max_rt;
    }
    if let Some(ref policy) = state.timeout_policy {
        ws.timeout_policy = policy.clone();
    }
    ws.pid = state.last_pid;
    if let Some(code) = state.last_exit_code {
        ws.exit_code = Some(code);
    }
}

/// Enforce per-workstream timeouts, marking timed-out workstreams.
pub(super) fn enforce_timeouts(
    workstreams: &mut [Workstream],
    processes: &HashMap<i64, Arc<Mutex<Option<Child>>>>,
) {
    for ws in workstreams.iter_mut() {
        if ws.lifecycle_state != "running" {
            continue;
        }
        let Some(start) = ws.start_time else {
            continue;
        };
        let elapsed = start.elapsed().as_secs();
        if elapsed < ws.max_runtime {
            continue;
        }

        let issue = ws.issue;
        println!(
            "[{issue}] Timed out after {}s, marking timed_out_resumable",
            ws.max_runtime
        );

        if ws.timeout_policy == INTERRUPT_PRESERVE_TIMEOUT_POLICY
            && let Some(proc_arc) = processes.get(&issue)
        {
            let mut guard = proc_arc.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut child) = *guard {
                let _ = child.kill();
                let _ = child.wait();
            }
        }

        ws.lifecycle_state = "timed_out_resumable".to_string();
        ws.end_time = Some(Instant::now());
        let _ = persist_state(ws);
    }
}

/// Terminate and mark interrupted all running workstreams.
pub(super) fn cleanup_running(
    workstreams: &mut [Workstream],
    processes: &HashMap<i64, Arc<Mutex<Option<Child>>>>,
) {
    for ws in workstreams.iter_mut() {
        if ws.lifecycle_state != "running" {
            continue;
        }
        let issue = ws.issue;
        if let Some(proc_arc) = processes.get(&issue) {
            let mut guard = proc_arc.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut child) = *guard {
                println!("[{issue}] Terminating PID {}...", ws.pid.unwrap_or(0));
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        ws.lifecycle_state = "interrupted_resumable".to_string();
        ws.end_time = Some(Instant::now());
        let _ = persist_state(ws);
    }
}

/// Poll workstream processes and categorize by status.
pub(super) fn get_status(
    workstreams: &mut [Workstream],
    processes: &HashMap<i64, Arc<Mutex<Option<Child>>>>,
) -> WorkstreamStatus {
    let mut status = WorkstreamStatus::default();

    for ws in workstreams.iter_mut() {
        let proc = processes.get(&ws.issue);

        let maybe_code = proc.and_then(|arc| {
            let mut guard = arc.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_mut().and_then(|child| {
                child
                    .try_wait()
                    .ok()
                    .flatten()
                    .map(|s| s.code().unwrap_or(-1))
            })
        });

        if let Some(code) = maybe_code {
            finalize_workstream(ws, code);
            if ws.lifecycle_state == "completed" {
                status.completed.push(ws.issue);
            } else {
                status.failed.push(ws.issue);
            }
        } else if proc.is_some() && ws.exit_code.is_none() {
            ws.lifecycle_state = "running".to_string();
            status.running.push(ws.issue);
        } else if ws.exit_code == Some(0) || ws.lifecycle_state == "completed" {
            status.completed.push(ws.issue);
        } else {
            status.failed.push(ws.issue);
        }
    }

    status
}

/// Clean up merged workstreams based on a JSON config and `gh pr` status.
pub(super) fn cleanup_merged(
    base_dir: &Path,
    state_dir: &Path,
    config_path: &str,
    dry_run: bool,
) -> Result<()> {
    let config_text = fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config: {config_path}"))?;
    let items: Vec<WorkstreamConfig> = serde_json::from_str(&config_text)?;

    let mut deleted_count = 0u32;
    let mut freed_bytes = 0u64;

    for item in &items {
        let issue = item.issue_id();
        let safe_id = sanitize_id(&issue.to_string());
        let work_dir = base_dir.join(format!("ws-{issue}"));
        let state_file = state_dir.join(format!("ws-{safe_id}.json"));

        // Check if PR is merged via gh CLI
        let is_merged = Command::new("gh")
            .args([
                "pr",
                "view",
                &item.branch,
                "--json",
                "state",
                "-q",
                ".state",
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .is_some_and(|s| s.trim() == "MERGED");

        if !is_merged {
            continue;
        }

        if work_dir.exists() {
            let dir_size = dir_size_bytes(&work_dir);
            if dry_run {
                println!(
                    "[{issue}] Would delete work dir ({:.0}MB)",
                    dir_size as f64 / (1024.0 * 1024.0)
                );
            } else {
                let _ = fs::remove_dir_all(&work_dir);
                let _ = fs::remove_file(&state_file);
                println!(
                    "[{issue}] Deleted work dir ({:.0}MB freed)",
                    dir_size as f64 / (1024.0 * 1024.0)
                );
            }
            freed_bytes += dir_size;
            deleted_count += 1;
        }
    }

    let freed_gb = freed_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    println!(
        "\n{}Summary:\n  Workstreams {}deleted: {deleted_count}\n  Disk space {}freed: {freed_gb:.2}GB",
        if dry_run { "DRY RUN " } else { "" },
        if dry_run { "would be " } else { "" },
        if dry_run { "would be " } else { "" },
    );

    if dry_run && deleted_count > 0 {
        println!("\nRun without --dry-run to actually delete these workstreams.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_persisted_state_roundtrip() {
        let state = PersistedState {
            issue: 42,
            branch: "feat/test".to_string(),
            lifecycle_state: "completed".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: PersistedState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.issue, 42);
        assert_eq!(parsed.branch, "feat/test");
        assert_eq!(parsed.lifecycle_state, "completed");
    }
}
