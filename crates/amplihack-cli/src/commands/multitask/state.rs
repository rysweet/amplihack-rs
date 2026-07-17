//! State persistence and lifecycle management for workstreams.

use super::models::*;
use super::persistence::persist_checkpoint;
use super::process_scope::{
    CurrentWorkflowScope, ProcessScopeConfig, ProcessScopeValidation, normalize_path,
    snapshot_for_pid, validate_process_scope,
};
use std::collections::HashMap;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::warn;

/// Status snapshot for all workstreams.
#[derive(Default)]
pub(super) struct WorkstreamStatus {
    pub running: Vec<i64>,
    pub completed: Vec<i64>,
    pub failed: Vec<i64>,
}

pub(super) fn finalize_workstream(ws: &mut Workstream, exit_code: i32) {
    if ws.end_time.is_none() {
        ws.end_time = Some(Instant::now());
    }
    ws.exit_code = Some(exit_code);

    if ws.lifecycle_state == "interrupted_resumable"
        || (ws.lifecycle_state == "timed_out_resumable"
            && ws.timeout_policy == INTERRUPT_PRESERVE_TIMEOUT_POLICY)
    {
        persist_checkpoint(ws);
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
    persist_checkpoint(ws);
}

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
    ws.workstream_scope = state.workstream_scope.clone();
    ws.process_scope = state.process_scope.clone();
    if let Some(code) = state.last_exit_code {
        ws.exit_code = Some(code);
    }
}

fn process_scope_validation(ws: &Workstream, pid: u32) -> ProcessScopeValidation {
    let current = CurrentWorkflowScope {
        repository: ws.workstream_scope.repository.clone(),
        repo_root: normalize_path(&ws.work_dir),
        workdir: normalize_path(&ws.work_dir),
        branch: ws.branch.clone(),
        issue_id: ws.issue.to_string(),
        work_item_id: ws.issue.to_string(),
        recipe_run_id: ws.workstream_scope.recipe_run_id.clone(),
        tree_id: ws.workstream_scope.tree_id.clone(),
        workstream_id: format!("ws-{}", ws.issue),
    };
    let snapshot = snapshot_for_pid(pid, &ws.work_dir);
    validate_process_scope(
        &current,
        &ws.workstream_scope,
        &ws.process_scope,
        &snapshot,
        &ProcessScopeConfig {
            max_age_seconds: ws.max_runtime as i64 + 3600,
        },
    )
}

fn process_scope_is_authoritative(ws: &Workstream, pid: u32, action: &str) -> bool {
    let validation = process_scope_validation(ws, pid);
    // Only ProcessScopeValidation::Valid is authoritative for monitor/closure paths.
    // Non-authoritative variants are display-only:
    // ProcessScopeValidation::MissingScope, Dead, PidReused, TooOld,
    // RepoMismatch, WorkdirMismatch, BranchMismatch, and WorkstreamMismatch.
    if validation.is_valid() {
        true
    } else {
        warn!(
            "[{}] Ignoring process for {action}: process_scope={}",
            ws.issue,
            validation.reason()
        );
        false
    }
}

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
        if start.elapsed().as_secs() < ws.max_runtime {
            continue;
        }

        let issue = ws.issue;
        println!(
            "[{issue}] Past max_runtime ({}s), marking timed_out_resumable",
            ws.max_runtime
        );

        let mut authoritative_process = true;
        if ws.timeout_policy == INTERRUPT_PRESERVE_TIMEOUT_POLICY
            && let Some(proc_arc) = processes.get(&issue)
        {
            let mut guard = proc_arc.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut child) = *guard {
                if process_scope_is_authoritative(ws, child.id(), "timeout") {
                    // Issue #867: interrupt only a genuinely idle child, never on elapsed alone.
                    if super::utils::child_is_idle(&ws.log_file) {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                } else {
                    authoritative_process = false;
                }
            }
        }
        if !authoritative_process {
            continue;
        }

        ws.lifecycle_state = "timed_out_resumable".to_string();
        ws.end_time = Some(Instant::now());
        persist_checkpoint(ws);
    }
}

pub(super) fn cleanup_running(
    workstreams: &mut [Workstream],
    processes: &HashMap<i64, Arc<Mutex<Option<Child>>>>,
) {
    for ws in workstreams.iter_mut() {
        if ws.lifecycle_state != "running" {
            continue;
        }
        let issue = ws.issue;
        let mut interrupted = processes.get(&issue).is_none();
        if let Some(proc_arc) = processes.get(&issue) {
            let mut guard = proc_arc.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut child) = *guard
                && process_scope_is_authoritative(ws, child.id(), "cleanup")
            {
                println!("[{issue}] Terminating PID {}...", ws.pid.unwrap_or(0));
                let _ = child.kill();
                let _ = child.wait();
                interrupted = true;
            }
        }
        if !interrupted {
            continue;
        }
        ws.lifecycle_state = "interrupted_resumable".to_string();
        ws.end_time = Some(Instant::now());
        persist_checkpoint(ws);
    }
}

pub(super) fn get_status(
    workstreams: &mut [Workstream],
    processes: &HashMap<i64, Arc<Mutex<Option<Child>>>>,
) -> WorkstreamStatus {
    let mut status = WorkstreamStatus::default();

    for ws in workstreams.iter_mut() {
        let proc = processes.get(&ws.issue);

        let authoritative_process = proc
            .and_then(|arc| {
                let guard = arc.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .as_ref()
                    .map(|child| process_scope_is_authoritative(ws, child.id(), "monitor"))
            })
            .unwrap_or(false);

        let maybe_code = if authoritative_process {
            proc.and_then(|arc| {
                let mut guard = arc.lock().unwrap_or_else(|e| e.into_inner());
                guard.as_mut().and_then(|child| {
                    child
                        .try_wait()
                        .ok()
                        .flatten()
                        .map(|s| s.code().unwrap_or(-1))
                })
            })
        } else {
            None
        };

        if let Some(code) = maybe_code {
            finalize_workstream(ws, code);
            if ws.lifecycle_state == "completed" {
                status.completed.push(ws.issue);
            } else {
                status.failed.push(ws.issue);
            }
        } else if proc.is_some() && authoritative_process && ws.exit_code.is_none() {
            ws.lifecycle_state = "running".to_string();
            status.running.push(ws.issue);
        } else if proc.is_some() && !authoritative_process && ws.exit_code.is_none() {
            warn!(
                "[{}] Workstream process is non-authoritative; leaving it display-only",
                ws.issue
            );
        } else if ws.exit_code == Some(0) || ws.lifecycle_state == "completed" {
            status.completed.push(ws.issue);
        } else {
            status.failed.push(ws.issue);
        }
    }

    status
}
