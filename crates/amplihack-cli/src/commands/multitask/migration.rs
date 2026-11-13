//! Guarded adoption of legacy issue-keyed state, retaining every recorded path.
use super::{
    issue::parse_optional,
    models::{PersistedState, WorkstreamConfig},
    runtime::{self, Runtime},
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub fn locate(config: &WorkstreamConfig) -> Result<Option<(PathBuf, Value)>> {
    let id = config.workstream_id.context("missing identity")?;
    let expected = runtime::state_path(id);
    let mut candidates = Vec::new();
    let dir = runtime::base_dir().join("state");
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_none_or(|s| s != "json")
            || path
                .file_name()
                .is_some_and(|s| s.to_string_lossy().contains(".progress."))
        {
            continue;
        }
        runtime::owned(&path, &dir)?;
        let value: Value = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("invalid saved state {}", path.display()))?;
        let same_id = value["workstream_id"] == serde_json::json!(id);
        let legacy = value.get("workstream_id").is_none()
            && !config.issue.is_null()
            && parse_optional(value.get("issue").unwrap_or(&Value::Null))?
                == parse_optional(&config.issue)?;
        if path == expected || same_id || legacy {
            candidates.push((path, value));
        }
    }
    ensure!(
        candidates.len() <= 1,
        "ambiguous legacy or duplicate saved state for leaf {}",
        config.key
    );
    Ok(candidates.pop())
}

pub fn adopt(
    config: WorkstreamConfig,
    repository: &Path,
    path: &Path,
    value: Value,
) -> Result<Runtime> {
    let legacy: PersistedState = serde_json::from_value(value)?;
    ensure!(
        !legacy.workstream_scope.repository.is_empty()
            && legacy.workstream_scope.repository == config.repository
            && !legacy.branch.is_empty()
            && legacy.branch == config.branch
            && legacy.workstream_scope.branch == legacy.branch,
        "legacy repository and branch ownership cannot be established"
    );
    let work = PathBuf::from(&legacy.work_dir);
    for p in [
        &work,
        &PathBuf::from(&legacy.log_file),
        &PathBuf::from(&legacy.progress_sidecar),
    ] {
        runtime::owned(p, &runtime::base_dir())?;
    }
    ensure!(
        work.canonicalize()? == Path::new(&legacy.workstream_scope.workdir).canonicalize()?,
        "legacy working-directory scope mismatch"
    );
    let out = amplihack_git::command()
        .args(["remote", "get-url", "origin"])
        .current_dir(&work)
        .output()?;
    ensure!(
        out.status.success(),
        "cannot verify legacy repository origin"
    );
    let origin = String::from_utf8(out.stdout)?;
    ensure!(
        amplihack_workflows::remote_repository::github_repository_from_remote_url(origin.trim())
            .as_deref()
            == Some(&config.repository),
        "legacy repository origin mismatch"
    );
    let out = amplihack_git::command()
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&work)
        .output()?;
    ensure!(
        out.status.success() && String::from_utf8(out.stdout)?.trim() == legacy.branch,
        "legacy branch ownership mismatch"
    );
    let mut state = Runtime::fresh(config, repository)?;
    state.state_file = path.to_owned();
    state.work_dir = work;
    state.log_file = legacy.log_file.clone().into();
    state.progress_sidecar = legacy.progress_sidecar.clone().into();
    state.process_scope = legacy.process_scope.clone();
    state.lifecycle = if legacy.lifecycle_state.is_empty() || legacy.lifecycle_state == "pending" {
        "pending"
    } else {
        "reconciliation_required"
    }
    .into();
    if state.lifecycle == "reconciliation_required" {
        state.diagnostic =
            "legacy execution lacks a validated completion; reconcile its recorded outcome".into();
    }
    state.legacy = Some(legacy);
    if state.config.classification == "Development" {
        let legacy = state.legacy.as_ref().unwrap();
        if !legacy.checkpoint_id.is_empty()
            && [
                "failed_resumable",
                "interrupted_resumable",
                "timed_out_resumable",
            ]
            .contains(&legacy.lifecycle_state.as_str())
        {
            if !legacy.worktree_path.is_empty() {
                runtime::owned(Path::new(&legacy.worktree_path), &runtime::base_dir())?;
            }
            state.lifecycle = "pending".into();
            state.diagnostic.clear();
        }
    }
    Ok(state)
}
