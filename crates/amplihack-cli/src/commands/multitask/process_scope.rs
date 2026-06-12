//! Process ownership validation for multitask monitor decisions.

use super::models::{ProcessScope, WorkstreamScope};
use chrono::{DateTime, Utc};
use std::fs;
use std::path::{Path, PathBuf};

/// Current workflow scope used to validate persisted workstream ownership.
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub pid: u32,
    pub alive: bool,
    pub workdir: String,
    pub process_started_at: String,
}

/// Validation knobs for stale process rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessScopeConfig {
    pub max_age_seconds: i64,
}

impl Default for ProcessScopeConfig {
    fn default() -> Self {
        Self {
            max_age_seconds: 24 * 60 * 60,
        }
    }
}

/// Fail-closed process ownership validation result.
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
    if workstream_scope.repository.is_empty()
        || workstream_scope.repo_root.is_empty()
        || workstream_scope.workdir.is_empty()
        || workstream_scope.branch.is_empty()
        || workstream_scope.issue_id.is_empty()
        || workstream_scope.workstream_id.is_empty()
        || process_scope.repository.is_empty()
        || process_scope.repo_root.is_empty()
        || process_scope.workdir.is_empty()
        || process_scope.branch.is_empty()
        || process_scope.issue_id.is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn full_scopes() -> (
        CurrentWorkflowScope,
        WorkstreamScope,
        ProcessScope,
        ProcessSnapshot,
        ProcessScopeConfig,
    ) {
        let current = CurrentWorkflowScope {
            repository: "owner/repo".to_string(),
            repo_root: "/repo".to_string(),
            workdir: "/repo/ws-754".to_string(),
            branch: "feat/issue-754".to_string(),
            issue_id: "754".to_string(),
            work_item_id: "754".to_string(),
            recipe_run_id: "run-1".to_string(),
            tree_id: "tree-1".to_string(),
            workstream_id: "ws-754".to_string(),
        };
        let workstream_scope = WorkstreamScope {
            repository: current.repository.clone(),
            repo_root: current.repo_root.clone(),
            workdir: current.workdir.clone(),
            branch: current.branch.clone(),
            base_ref: "main".to_string(),
            issue_id: current.issue_id.clone(),
            work_item_id: current.work_item_id.clone(),
            recipe: "default-workflow".to_string(),
            recipe_run_id: current.recipe_run_id.clone(),
            tree_id: current.tree_id.clone(),
            workstream_id: current.workstream_id.clone(),
            expected_title_prefix: "Fix scoped monitor closure".to_string(),
            started_at: Utc::now().to_rfc3339(),
        };
        let process_scope = ProcessScope {
            pid: Some(4242),
            repository: current.repository.clone(),
            repo_root: current.repo_root.clone(),
            workdir: current.workdir.clone(),
            branch: current.branch.clone(),
            issue_id: current.issue_id.clone(),
            work_item_id: current.work_item_id.clone(),
            recipe_run_id: current.recipe_run_id.clone(),
            tree_id: current.tree_id.clone(),
            workstream_id: current.workstream_id.clone(),
            process_started_at: "start-1".to_string(),
            recorded_at: Utc::now().to_rfc3339(),
        };
        let snapshot = ProcessSnapshot {
            pid: 4242,
            alive: true,
            workdir: current.workdir.clone(),
            process_started_at: "start-1".to_string(),
        };
        (
            current,
            workstream_scope,
            process_scope,
            snapshot,
            ProcessScopeConfig::default(),
        )
    }

    #[test]
    fn matching_live_process_with_full_scope_is_valid() {
        let (current, workstream_scope, process_scope, snapshot, config) = full_scopes();
        assert_eq!(
            validate_process_scope(
                &current,
                &workstream_scope,
                &process_scope,
                &snapshot,
                &config
            ),
            ProcessScopeValidation::Valid
        );
        assert_eq!(ProcessScopeValidation::Valid.reason(), "valid");
    }

    #[test]
    fn legacy_state_without_process_scope_is_non_authoritative() {
        let (current, workstream_scope, _process_scope, snapshot, config) = full_scopes();
        assert_eq!(
            validate_process_scope(
                &current,
                &workstream_scope,
                &ProcessScope::default(),
                &snapshot,
                &config
            ),
            ProcessScopeValidation::MissingScope
        );
    }

    #[test]
    fn dead_reused_or_too_old_process_records_are_rejected() {
        let (current, workstream_scope, mut process_scope, mut snapshot, config) = full_scopes();
        snapshot.alive = false;
        assert_eq!(
            validate_process_scope(
                &current,
                &workstream_scope,
                &process_scope,
                &snapshot,
                &config
            ),
            ProcessScopeValidation::Dead
        );

        snapshot.alive = true;
        snapshot.process_started_at = "different-start".to_string();
        assert_eq!(
            validate_process_scope(
                &current,
                &workstream_scope,
                &process_scope,
                &snapshot,
                &config
            ),
            ProcessScopeValidation::PidReused
        );

        snapshot.process_started_at = process_scope.process_started_at.clone();
        process_scope.recorded_at = "2020-01-01T00:00:00Z".to_string();
        assert_eq!(
            validate_process_scope(
                &current,
                &workstream_scope,
                &process_scope,
                &snapshot,
                &config
            ),
            ProcessScopeValidation::TooOld
        );
    }

    #[test]
    fn repo_workdir_branch_and_workstream_mismatches_are_rejected() {
        let (mut current, workstream_scope, process_scope, snapshot, config) = full_scopes();
        current.repository = "other/repo".to_string();
        assert_eq!(
            validate_process_scope(
                &current,
                &workstream_scope,
                &process_scope,
                &snapshot,
                &config
            ),
            ProcessScopeValidation::RepoMismatch
        );

        let (mut current, workstream_scope, process_scope, snapshot, config) = full_scopes();
        current.workdir = "/other".to_string();
        assert_eq!(
            validate_process_scope(
                &current,
                &workstream_scope,
                &process_scope,
                &snapshot,
                &config
            ),
            ProcessScopeValidation::WorkdirMismatch
        );

        let (mut current, workstream_scope, process_scope, snapshot, config) = full_scopes();
        current.branch = "other".to_string();
        assert_eq!(
            validate_process_scope(
                &current,
                &workstream_scope,
                &process_scope,
                &snapshot,
                &config
            ),
            ProcessScopeValidation::BranchMismatch
        );

        let (mut current, workstream_scope, process_scope, snapshot, config) = full_scopes();
        current.workstream_id = "other".to_string();
        assert_eq!(
            validate_process_scope(
                &current,
                &workstream_scope,
                &process_scope,
                &snapshot,
                &config
            ),
            ProcessScopeValidation::WorkstreamMismatch
        );
    }
}
