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
use tracing::warn;

pub(super) fn read_state(path: &Path) -> Result<Option<PersistedState>> {
    use std::io::Read;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(anyhow::anyhow!("read state {}: {error}", path.display())),
    };
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "state is not a regular file: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(serde_json::from_slice(&bytes).map_err(|error| {
        anyhow::anyhow!("invalid state {}: {error}", path.display())
    })?))
}

pub(super) fn persist_state(ws: &Workstream) -> Result<()> {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let workstream_scope = ws.workstream_scope.clone();
    if workstream_scope.base_ref.is_empty() && !workstream_scope.repository.is_empty() {
        warn!("[{}] workstream_scope missing base_ref", ws.key);
    }

    let existing = read_state(&ws.state_file)?;
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
        source_repository: ws.source_repository.clone(),
        classification: ws.classification.clone(),
        dependencies: ws.dependencies.clone(),
        create_issue: ws.create_issue,
        tracking_reference: ws.tracking_reference.clone(),
        result: ws.result.clone(),

        key: ws.key.to_string(),
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
