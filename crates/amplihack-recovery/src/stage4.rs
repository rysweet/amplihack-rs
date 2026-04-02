use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use tracing::{info, warn};

use crate::models::{AtlasProvenance, RecoveryBlocker, Stage4AtlasRun, StageStatus};

/// Configuration for the code-atlas adapter.
#[derive(Clone, Debug)]
pub struct CodeAtlasAdapter {
    pub command: String,
    pub timeout_secs: u64,
    pub max_attempts: u32,
    pub backoff_seconds: u64,
}

impl Default for CodeAtlasAdapter {
    fn default() -> Self {
        Self {
            command: "code-atlas".into(),
            timeout_secs: 120,
            max_attempts: 3,
            backoff_seconds: 2,
        }
    }
}

/// Determine the atlas target directory and provenance.
pub fn determine_atlas_target(
    repo_path: &Path,
    worktree_path: Option<&Path>,
) -> (PathBuf, AtlasProvenance) {
    if let Some(wt) = worktree_path {
        if wt.exists() {
            return (wt.to_path_buf(), AtlasProvenance::IsolatedWorktree);
        }
        warn!("stage4: worktree path does not exist, falling back");
    }

    if repo_path.exists() {
        (
            repo_path.to_path_buf(),
            AtlasProvenance::CurrentTreeReadOnly,
        )
    } else {
        (repo_path.to_path_buf(), AtlasProvenance::Blocked)
    }
}

/// Execute the code-atlas command with retry and exponential backoff.
fn execute_with_retry(adapter: &CodeAtlasAdapter, target: &Path) -> Result<(bool, Vec<String>)> {
    let mut artifacts = Vec::new();
    let mut last_err = None;

    for attempt in 1..=adapter.max_attempts {
        info!("stage4: atlas attempt {attempt}/{}", adapter.max_attempts);

        let result = Command::new(&adapter.command)
            .arg(target)
            .current_dir(target)
            .output();

        match result {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        artifacts.push(trimmed.to_string());
                    }
                }
                return Ok((true, artifacts));
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("stage4: attempt {attempt} failed: {stderr}");
                last_err = Some(stderr.to_string());
            }
            Err(e) => {
                warn!("stage4: attempt {attempt} could not execute: {e}");
                last_err = Some(e.to_string());
            }
        }

        if attempt < adapter.max_attempts {
            let delay = adapter.backoff_seconds * 2u64.pow(attempt - 1);
            info!("stage4: backing off {delay}s before retry");
            thread::sleep(Duration::from_secs(delay));
        }
    }

    Err(anyhow::anyhow!(
        "atlas failed after {} attempts: {}",
        adapter.max_attempts,
        last_err.unwrap_or_default()
    ))
}

/// Stage 4: code atlas execution with retry/backoff.
pub fn run_stage4(repo_path: &Path, worktree_path: Option<&Path>) -> Result<Stage4AtlasRun> {
    info!("stage4: starting code-atlas execution");

    let (target, provenance) = determine_atlas_target(repo_path, worktree_path);

    if provenance == AtlasProvenance::Blocked {
        return Ok(Stage4AtlasRun {
            status: StageStatus::Blocked,
            skill: "code-atlas".into(),
            provenance,
            artifacts: vec![],
            blockers: vec![RecoveryBlocker {
                stage: 4,
                code: "ATLAS_TARGET_BLOCKED".into(),
                message: "no valid atlas target directory".into(),
                retryable: false,
            }],
        });
    }

    let adapter = CodeAtlasAdapter::default();
    let mut blockers = Vec::new();

    let (success, artifacts) = match execute_with_retry(&adapter, &target) {
        Ok(result) => result,
        Err(e) => {
            blockers.push(RecoveryBlocker {
                stage: 4,
                code: "ATLAS_EXEC_FAILED".into(),
                message: e.to_string(),
                retryable: true,
            });
            (false, vec![])
        }
    };

    let status = if success && blockers.is_empty() {
        StageStatus::Completed
    } else {
        StageStatus::Blocked
    };

    Ok(Stage4AtlasRun {
        status,
        skill: "code-atlas".into(),
        provenance,
        artifacts,
        blockers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn determine_target_with_valid_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();
        let (target, prov) = determine_atlas_target(tmp.path(), Some(wt.path()));
        assert_eq!(target, wt.path());
        assert_eq!(prov, AtlasProvenance::IsolatedWorktree);
    }

    #[test]
    fn determine_target_worktree_missing_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (target, prov) =
            determine_atlas_target(tmp.path(), Some(Path::new("/nonexistent/worktree")));
        assert_eq!(target, tmp.path());
        assert_eq!(prov, AtlasProvenance::CurrentTreeReadOnly);
    }

    #[test]
    fn determine_target_no_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let (target, prov) = determine_atlas_target(tmp.path(), None);
        assert_eq!(target, tmp.path());
        assert_eq!(prov, AtlasProvenance::CurrentTreeReadOnly);
    }

    #[test]
    fn determine_target_repo_missing() {
        let (_, prov) = determine_atlas_target(Path::new("/nonexistent/repo"), None);
        assert_eq!(prov, AtlasProvenance::Blocked);
    }

    #[test]
    fn adapter_default_values() {
        let a = CodeAtlasAdapter::default();
        assert_eq!(a.max_attempts, 3);
        assert_eq!(a.backoff_seconds, 2);
        assert_eq!(a.timeout_secs, 120);
    }

    #[test]
    fn run_stage4_blocked_provenance() {
        let result = run_stage4(Path::new("/nonexistent/repo"), None).unwrap();
        assert_eq!(result.status, StageStatus::Blocked);
        assert_eq!(result.provenance, AtlasProvenance::Blocked);
        assert!(!result.blockers.is_empty());
    }

    #[test]
    fn run_stage4_command_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let result = run_stage4(tmp.path(), None).unwrap();
        // code-atlas binary won't exist in test env
        assert_eq!(result.status, StageStatus::Blocked);
        assert!(
            result
                .blockers
                .iter()
                .any(|b| b.code == "ATLAS_EXEC_FAILED")
        );
    }
}
