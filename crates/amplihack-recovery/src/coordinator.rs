use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use tracing::info;

use crate::models::RecoveryRun;
use crate::results::write_recovery_ledger;
use crate::stage1::run_stage1;
use crate::stage2::run_stage2;
use crate::stage3::run_stage3;
use crate::stage4::run_stage4;

/// Run the full 4-stage recovery pipeline.
///
/// Stages execute sequentially; blockers are aggregated into the final
/// `RecoveryRun`. An `output_path` is optionally used to persist the
/// recovery ledger as JSON.
pub fn run_recovery(
    repo_path: &Path,
    output_path: Option<&Path>,
    worktree_path: Option<&Path>,
    min_cycles: u32,
    max_cycles: u32,
) -> Result<RecoveryRun> {
    info!("recovery: starting 4-stage pipeline");
    let started_at = Utc::now();

    let mut run = RecoveryRun {
        repo_path: repo_path.to_path_buf(),
        started_at,
        finished_at: None,
        protected_staged_files: vec![],
        stage1: None,
        stage2: None,
        stage3: None,
        stage4: None,
        blockers: vec![],
    };

    // Stage 1
    info!("recovery: stage 1");
    let s1 = run_stage1(repo_path, worktree_path)?;
    run.protected_staged_files.clone_from(&s1.protected_staged_files);
    run.blockers.extend(s1.blockers.iter().cloned());
    run.stage1 = Some(s1);

    // Stage 2
    info!("recovery: stage 2");
    let s2 = run_stage2(repo_path, &run.protected_staged_files)?;
    run.blockers.extend(s2.blockers.iter().cloned());
    run.stage2 = Some(s2.clone());

    // Stage 3
    info!("recovery: stage 3");
    let s3 = run_stage3(&s2, repo_path, worktree_path, min_cycles, max_cycles)?;
    run.blockers.extend(s3.blockers.iter().cloned());
    run.stage3 = Some(s3);

    // Stage 4
    info!("recovery: stage 4");
    let s4 = run_stage4(repo_path, worktree_path)?;
    run.blockers.extend(s4.blockers.iter().cloned());
    run.stage4 = Some(s4);

    run.finished_at = Some(Utc::now());

    // Persist ledger if output_path given
    if let Some(out) = output_path {
        write_recovery_ledger(&run, out)?;
    }

    info!(
        "recovery: finished with {} blocker(s)",
        run.blockers.len()
    );
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

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
        std::fs::write(dir.join("README.md"), "init").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn full_pipeline_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_git_repo(repo);
        // Create pyproject.toml so stage3 validators pass
        std::fs::write(repo.join("pyproject.toml"), "[tool.pytest]").unwrap();

        let result = run_recovery(repo, None, None, 3, 6).unwrap();
        assert!(result.stage1.is_some());
        assert!(result.stage2.is_some());
        assert!(result.stage3.is_some());
        assert!(result.stage4.is_some());
        assert!(result.finished_at.is_some());
    }

    #[test]
    fn pipeline_writes_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_git_repo(repo);
        std::fs::write(repo.join("pyproject.toml"), "[tool.pytest]").unwrap();

        let ledger = repo.join("recovery-ledger.json");
        let _result = run_recovery(repo, Some(&ledger), None, 3, 6).unwrap();
        assert!(ledger.exists());

        let content = std::fs::read_to_string(&ledger).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed.get("repo_path").is_some());
    }

    #[test]
    fn pipeline_aggregates_blockers() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_git_repo(repo);
        // Create dirty .claude to trigger stage1 blocker
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        std::fs::write(repo.join(".claude/config.json"), "{}").unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[tool.pytest]").unwrap();

        let result = run_recovery(repo, None, None, 3, 6).unwrap();
        assert!(result
            .blockers
            .iter()
            .any(|b| b.code == "CLAUDE_DIRTY"));
    }
}
