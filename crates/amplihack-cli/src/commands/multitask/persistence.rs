//! State-file persistence I/O for workstreams.
//!
//! Serializes and deserializes `PersistedState` to disk and surfaces (never
//! swallows) failures, so a missing state file stays distinguishable from a
//! corrupt one and a lost checkpoint write is never mistaken for an up-to-date
//! one.

use super::models::*;
use super::utils::atomic_write;
use anyhow::Result;
use chrono::Utc;
use std::fs;
use std::path::Path;
use tracing::{error, warn};

pub(super) fn load_state(state_file: &Path) -> Option<PersistedState> {
    // Missing != corrupt: an absent state file is the normal first-run
    // condition and stays silent; a present-but-unreadable/unparseable file is
    // surfaced so a real failure is never mistaken for "no state".
    let text = match fs::read_to_string(state_file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            warn!(
                path = %state_file.display(),
                error = %e,
                "failed to read workstream state file; treating as absent"
            );
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(state) => Some(state),
        Err(e) => {
            error!(
                path = %state_file.display(),
                error = %e,
                "workstream state file is present but corrupt; ignoring it (this is NOT the same as a missing file)"
            );
            None
        }
    }
}

pub(super) fn persist_state(ws: &Workstream) -> Result<()> {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let workstream_scope = ws.workstream_scope.clone();
    if workstream_scope.base_ref.is_empty() && !workstream_scope.repository.is_empty() {
        warn!("[{}] workstream_scope missing base_ref", ws.issue);
    }

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
        workstream_scope,
        process_scope: ws.process_scope.clone(),
        created_at,
        updated_at: now,
        resume_context: existing.and_then(|s| s.resume_context),
    };

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write(&ws.state_file, json.as_bytes())?;
    Ok(())
}

/// Persist a workstream checkpoint, surfacing (never swallowing) write failures.
///
/// A failed checkpoint write leaves nothing on disk while the caller believes
/// the workstream is resumable. Logging the failure keeps a lost checkpoint
/// distinguishable from a genuinely up-to-date one.
pub(super) fn persist_checkpoint(ws: &Workstream) {
    if let Err(e) = persist_state(ws) {
        error!(
            issue = ws.issue,
            path = %ws.state_file.display(),
            error = %e,
            "failed to persist workstream checkpoint; on-disk state is now stale or missing despite the workstream being marked resumable"
        );
    }
}
