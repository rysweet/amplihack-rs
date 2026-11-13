//! Adopt compatible persisted state; execution transitions live in dispatch.
use super::models::*;
#[cfg(test)]
use super::persistence::persist_checkpoint;
#[cfg(test)]
use super::process_scope::{
    CurrentWorkflowScope, ProcessScopeConfig, ProcessScopeValidation, normalize_path,
    snapshot_for_pid, validate_process_scope,
};
#[cfg(test)]
use std::{
    collections::HashMap,
    process::Child,
    sync::{Arc, Mutex},
    time::Instant,
};
use tracing::warn;

pub(super) fn apply_saved_state(ws: &mut Workstream, state: &PersistedState) {
    // SR7 (issue #2553): a state file is untrusted input. The in-memory,
    // config-derived key owns every path; a key read back from disk is only
    // ever compared, never used to construct one, so anything with write
    // access to the state directory cannot point a run at another run's work.
    if !state.key.is_empty() && state.key != ws.key.as_str() {
        warn!(
            "[{}] state file records a different workstream identity {:?}; ignoring it \
             and keeping the configured paths",
            ws.key, state.key
        );
        return;
    }
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
    if ws.issue.is_none() {
        ws.issue = state.issue;
    }
    ws.tracking_reference = state.tracking_reference.clone();
    ws.result = state.result.clone();
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

#[cfg(test)]
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

#[cfg(test)]
fn process_scope_validation(ws: &Workstream, pid: u32) -> ProcessScopeValidation {
    let current = CurrentWorkflowScope {
        repository: ws.workstream_scope.repository.clone(),
        repo_root: normalize_path(&ws.work_dir),
        workdir: normalize_path(&ws.work_dir),
        branch: ws.branch.clone(),
        // Issue #2553: `issue_id` is empty for an untracked workstream, so the
        // presence gate reads `work_item_id`, which carries the identity key
        // and is never empty.
        issue_id: ws.issue.map(|n| n.to_string()).unwrap_or_default(),
        work_item_id: ws.key.to_string(),
        recipe_run_id: ws.workstream_scope.recipe_run_id.clone(),
        tree_id: ws.workstream_scope.tree_id.clone(),
        workstream_id: format!("ws-{}", ws.key),
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

#[cfg(test)]
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
            ws.key,
            validation.reason()
        );
        false
    }
}

#[cfg(test)]
pub(super) fn cleanup_running(
    workstreams: &mut [Workstream],
    processes: &HashMap<WorkstreamKey, Arc<Mutex<Option<Child>>>>,
) {
    for ws in workstreams.iter_mut() {
        if ws.lifecycle_state != "running" {
            continue;
        }
        let key = ws.key.clone();
        let mut interrupted = processes.get(&key).is_none();
        if let Some(proc_arc) = processes.get(&key) {
            let mut guard = proc_arc.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut child) = *guard
                && process_scope_is_authoritative(ws, child.id(), "cleanup")
            {
                println!("[{key}] Terminating PID {}...", ws.pid.unwrap_or(0));
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
