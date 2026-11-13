//! Process ownership validation for multitask monitor decisions.

#[cfg(test)]
use super::models::{ProcessScope, WorkstreamScope};
#[cfg(test)]
use chrono::{DateTime, Utc};
use std::fs;
use std::path::{Path, PathBuf};

/// Current workflow scope used to validate persisted workstream ownership.
#[cfg(test)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CurrentWorkflowScope {
    pub repository: String,
    pub repo_root: String,
    pub workdir: String,
    pub branch: String,
    pub issue_id: String,
    pub work_item_id: String,
    pub recipe_run_id: String,
    pub tree_id: String,
    pub workstream_id: String,
}

/// Runtime process metadata observed during monitor polling.
#[cfg(test)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub pid: u32,
    pub alive: bool,
    pub workdir: String,
    pub process_started_at: String,
}

/// Validation knobs for stale process rejection.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessScopeConfig {
    pub max_age_seconds: i64,
}

#[cfg(test)]
impl Default for ProcessScopeConfig {
    fn default() -> Self {
        Self {
            max_age_seconds: 24 * 60 * 60,
        }
    }
}

/// Fail-closed process ownership validation result.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessScopeValidation {
    Valid,
    MissingScope,
    Dead,
    PidReused,
    TooOld,
    RepoMismatch,
    WorkdirMismatch,
    BranchMismatch,
    WorkstreamMismatch,
}

#[cfg(test)]
impl ProcessScopeValidation {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::MissingScope => "missing_scope",
            Self::Dead => "dead",
            Self::PidReused => "pid_reused",
            Self::TooOld => "too_old",
            Self::RepoMismatch => "repo_mismatch",
            Self::WorkdirMismatch => "workdir_mismatch",
            Self::BranchMismatch => "branch_mismatch",
            Self::WorkstreamMismatch => "workstream_mismatch",
        }
    }

    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

/// Validate persisted process scope against current workflow and runtime facts.
#[cfg(test)]
pub fn validate_process_scope(
    current: &CurrentWorkflowScope,
    workstream_scope: &WorkstreamScope,
    process_scope: &ProcessScope,
    snapshot: &ProcessSnapshot,
    config: &ProcessScopeConfig,
) -> ProcessScopeValidation {
    let Some(pid) = process_scope.pid else {
        return ProcessScopeValidation::MissingScope;
    };
    // Issue #2553: the presence gate reads `work_item_id` rather than
    // `issue_id`, because a workstream may legitimately track no issue. The
    // gate keeps its width: `work_item_id` carries the identity key, is never
    // empty, and is now checked on BOTH scopes where the base checked it on
    // neither. Every equality comparison below, `issue_id` included, is
    // unchanged.
    if workstream_scope.repository.is_empty()
        || workstream_scope.repo_root.is_empty()
        || workstream_scope.workdir.is_empty()
        || workstream_scope.branch.is_empty()
        || workstream_scope.work_item_id.is_empty()
        || workstream_scope.workstream_id.is_empty()
        || process_scope.repository.is_empty()
        || process_scope.repo_root.is_empty()
        || process_scope.workdir.is_empty()
        || process_scope.branch.is_empty()
        || process_scope.work_item_id.is_empty()
        || process_scope.workstream_id.is_empty()
        || process_scope.process_started_at.is_empty()
        || process_scope.recorded_at.is_empty()
    {
        return ProcessScopeValidation::MissingScope;
    }
    if !snapshot.alive {
        return ProcessScopeValidation::Dead;
    }
    if snapshot.pid != pid {
        return ProcessScopeValidation::PidReused;
    }
    if !snapshot.process_started_at.is_empty()
        && snapshot.process_started_at != process_scope.process_started_at
    {
        return ProcessScopeValidation::PidReused;
    }
    if is_too_old(&process_scope.recorded_at, config.max_age_seconds) {
        return ProcessScopeValidation::TooOld;
    }
    if current.repository != workstream_scope.repository
        || current.repository != process_scope.repository
        || current.repo_root != workstream_scope.repo_root
        || current.repo_root != process_scope.repo_root
    {
        return ProcessScopeValidation::RepoMismatch;
    }
    if current.workdir != workstream_scope.workdir
        || current.workdir != process_scope.workdir
        || (!snapshot.workdir.is_empty() && snapshot.workdir != process_scope.workdir)
    {
        return ProcessScopeValidation::WorkdirMismatch;
    }
    if current.branch != workstream_scope.branch || current.branch != process_scope.branch {
        return ProcessScopeValidation::BranchMismatch;
    }
    if current.issue_id != workstream_scope.issue_id
        || current.issue_id != process_scope.issue_id
        || current.work_item_id != workstream_scope.work_item_id
        || current.work_item_id != process_scope.work_item_id
        || current.recipe_run_id != workstream_scope.recipe_run_id
        || current.recipe_run_id != process_scope.recipe_run_id
        || current.tree_id != workstream_scope.tree_id
        || current.tree_id != process_scope.tree_id
        || current.workstream_id != workstream_scope.workstream_id
        || current.workstream_id != process_scope.workstream_id
    {
        return ProcessScopeValidation::WorkstreamMismatch;
    }
    ProcessScopeValidation::Valid
}

#[cfg(test)]
pub fn snapshot_for_pid(pid: u32, workdir: &Path) -> ProcessSnapshot {
    ProcessSnapshot {
        pid,
        alive: process_alive(pid),
        workdir: normalize_path(workdir),
        process_started_at: process_start_metadata(pid).unwrap_or_default(),
    }
}

pub fn process_start_metadata(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(") ")
            .and_then(|(_, rest)| rest.split_whitespace().nth(19))
            .map(str::to_string)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

#[cfg(test)]
pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

pub fn normalize_path(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
fn is_too_old(recorded_at: &str, max_age_seconds: i64) -> bool {
    DateTime::parse_from_rfc3339(recorded_at)
        .map(|dt| {
            Utc::now()
                .signed_duration_since(dt.with_timezone(&Utc))
                .num_seconds()
        })
        .map(|age| age > max_age_seconds)
        .unwrap_or(true)
}
