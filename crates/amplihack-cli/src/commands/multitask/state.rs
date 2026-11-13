//! Adopt compatible persisted state; execution transitions live in dispatch.
use super::models::*;
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
