use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::models::{FixVerifyMode, RecoveryBlocker, Stage1Result, StageStatus};

/// Capture the list of staged files via `git diff --cached --name-only`.
fn capture_protected_staged_files(repo_path: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(repo_path)
        .output()
        .context("failed to run git diff --cached")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    Ok(files)
}

/// Check whether `.claude` has uncommitted changes.
fn has_dirty_claude(repo_path: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain", "--", ".claude"])
        .current_dir(repo_path)
        .output()
        .context("failed to run git status --porcelain")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(!stdout.trim().is_empty())
}

/// Determine `FixVerifyMode` based on the presence of a worktree path.
fn determine_mode(worktree_path: Option<&Path>) -> FixVerifyMode {
    if worktree_path.is_some() {
        FixVerifyMode::IsolatedWorktree
    } else {
        FixVerifyMode::ReadOnly
    }
}

/// Stage 1: protect staged files and detect `.claude` changes.
pub fn run_stage1(repo_path: &Path, worktree_path: Option<&Path>) -> Result<Stage1Result> {
    info!("stage1: starting protected-staging check");

    let mut actions = Vec::new();
    let mut blockers = Vec::new();

    let protected = capture_protected_staged_files(repo_path)?;
    actions.push(format!("captured {} staged file(s)", protected.len()));

    let claude_dirty = has_dirty_claude(repo_path)?;
    if claude_dirty {
        warn!("stage1: .claude directory has uncommitted changes");
        blockers.push(RecoveryBlocker {
            stage: 1,
            code: "CLAUDE_DIRTY".into(),
            message: ".claude has uncommitted changes".into(),
            retryable: false,
        });
    } else {
        actions.push(".claude clean".into());
    }

    let mode = determine_mode(worktree_path);
    let status = if blockers.is_empty() {
        StageStatus::Completed
    } else {
        StageStatus::Blocked
    };

    Ok(Stage1Result {
        status,
        mode,
        protected_staged_files: protected,
        actions,
        blockers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_git_repo(dir: &Path) {
        Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .unwrap();
        // Need at least one commit for diff to work
        std::fs::write(dir.join("README.md"), "init").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@test",
                "commit",
                "-m",
                "init",
            ])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn clean_repo_completes() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_git_repo(repo);

        let result = run_stage1(repo, None).unwrap();
        assert_eq!(result.status, StageStatus::Completed);
        assert!(result.blockers.is_empty());
    }

    #[test]
    fn dirty_claude_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_git_repo(repo);

        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        std::fs::write(repo.join(".claude/config.json"), "{}").unwrap();

        let result = run_stage1(repo, None).unwrap();
        assert_eq!(result.status, StageStatus::Blocked);
        assert_eq!(result.blockers[0].code, "CLAUDE_DIRTY");
    }

    #[test]
    fn protected_files_captured() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_git_repo(repo);

        std::fs::write(repo.join("staged.txt"), "data").unwrap();
        Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(repo)
            .output()
            .unwrap();

        let result = run_stage1(repo, None).unwrap();
        assert!(
            result
                .protected_staged_files
                .contains(&"staged.txt".to_string())
        );
    }

    #[test]
    fn worktree_sets_isolated_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_git_repo(repo);

        let wt = tempfile::tempdir().unwrap();
        let result = run_stage1(repo, Some(wt.path())).unwrap();
        assert_eq!(result.mode, FixVerifyMode::IsolatedWorktree);
    }

    #[test]
    fn no_worktree_sets_readonly_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_git_repo(repo);

        let result = run_stage1(repo, None).unwrap();
        assert_eq!(result.mode, FixVerifyMode::ReadOnly);
    }
}
