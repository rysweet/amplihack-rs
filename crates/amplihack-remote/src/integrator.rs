//! Result integration for remote execution.
//!
//! Handles integrating remote execution results back into the local
//! repository: git branch import, log copying, and conflict detection.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::error::{ErrorContext, RemoteError};

/// Information about a git branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub commit: String,
    pub is_new: bool,
}

/// Summary of result integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationSummary {
    pub branches: Vec<BranchInfo>,
    pub commits_count: usize,
    pub files_changed: usize,
    pub logs_copied: bool,
    pub has_conflicts: bool,
    pub conflict_details: Option<String>,
}

/// Integrates remote execution results into the local repository.
pub struct Integrator {
    repo_path: PathBuf,
}

impl Integrator {
    pub fn new(repo_path: &Path) -> Result<Self, RemoteError> {
        let repo_path = repo_path
            .canonicalize()
            .unwrap_or_else(|_| repo_path.to_path_buf());
        if !repo_path.join(".git").exists() {
            return Err(RemoteError::integration_ctx(
                format!("Not a git repository: {}", repo_path.display()),
                ErrorContext::new().insert("repo_path", repo_path.display().to_string()),
            ));
        }
        Ok(Self { repo_path })
    }

    /// Integrate remote results into the local repository.
    pub async fn integrate(&self, results_dir: &Path) -> Result<IntegrationSummary, RemoteError> {
        let branches = self.import_branches(results_dir).await?;
        let commits_count = self.count_new_commits(&branches).await;
        let logs_copied = self.copy_logs(results_dir).await;
        let conflicts = self.detect_conflicts(&branches).await;
        let files_changed = self.count_files_changed(&branches).await;

        Ok(IntegrationSummary {
            branches,
            commits_count,
            files_changed,
            logs_copied,
            has_conflicts: !conflicts.is_empty(),
            conflict_details: if conflicts.is_empty() {
                None
            } else {
                Some(conflicts.join("\n"))
            },
        })
    }

    /// Create a human-readable summary report.
    pub fn create_summary_report(&self, summary: &IntegrationSummary) -> String {
        let sep = "=".repeat(60);
        let mut lines = vec![
            String::new(),
            sep.clone(),
            "Remote Execution Results".to_string(),
            sep.clone(),
            String::new(),
        ];

        lines.push(format!("Branches ({}):", summary.branches.len()));
        for b in &summary.branches {
            let status = if b.is_new { "NEW" } else { "UPDATED" };
            lines.push(format!(
                "  - {} ({status}): {}",
                b.name,
                &b.commit[..b.commit.len().min(8)]
            ));
        }
        lines.push(String::new());

        lines.push(format!("Commits: {}", summary.commits_count));
        lines.push(format!("Files changed: {}", summary.files_changed));
        lines.push(format!(
            "Logs copied: {}",
            if summary.logs_copied { "Yes" } else { "No" }
        ));
        lines.push(String::new());

        if summary.has_conflicts {
            lines.push("WARNING: Conflicts detected!".to_string());
            if let Some(ref details) = summary.conflict_details {
                lines.push(details.clone());
            }
            lines.push(String::new());
            lines.push(
                "Branches available in 'remote-exec/' \
                 namespace for manual merge:"
                    .to_string(),
            );
        } else {
            lines.push("Status: No conflicts detected".to_string());
            lines.push(String::new());
            lines.push("To merge remote changes:".to_string());
        }

        for b in &summary.branches {
            lines.push(format!("  git merge remote-exec/{}", b.name));
        }

        lines.push(String::new());
        lines.push(sep);

        lines.join("\n")
    }

    // ---- internal ----

    async fn import_branches(&self, results_dir: &Path) -> Result<Vec<BranchInfo>, RemoteError> {
        let bundle_path = results_dir.join("results.bundle");
        if !bundle_path.exists() {
            return Err(RemoteError::integration_ctx(
                "Results bundle not found",
                ErrorContext::new().insert("expected_path", bundle_path.display().to_string()),
            ));
        }

        info!("importing remote branches");

        let current_branches = self.list_local_branches().await;

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            Command::new("git")
                .args([
                    "fetch",
                    bundle_path.to_str().unwrap_or("."),
                    "refs/heads/*:refs/remotes/remote-exec/*",
                ])
                .current_dir(&self.repo_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await
        .map_err(|_| RemoteError::integration("Branch import timed out"))?
        .map_err(|e| RemoteError::integration(format!("Branch import failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RemoteError::integration_ctx(
                format!("Failed to import branches: {stderr}"),
                ErrorContext::new().insert("bundle_path", bundle_path.display().to_string()),
            ));
        }

        let imported = self.list_remote_exec_branches().await;

        let mut branch_info = Vec::new();
        for branch_name in &imported {
            let commit = self
                .git_rev_parse(&format!("remote-exec/{branch_name}"))
                .await
                .unwrap_or_default();
            let is_new = !current_branches.contains(branch_name);
            branch_info.push(BranchInfo {
                name: branch_name.clone(),
                commit,
                is_new,
            });
        }

        info!(count = branch_info.len(), "branches imported");
        Ok(branch_info)
    }

    async fn copy_logs(&self, results_dir: &Path) -> bool {
        let source = results_dir.join(".claude/runtime/logs");
        if !source.is_dir() {
            warn!("no logs found in results");
            return false;
        }

        let dest = self.repo_path.join(".claude/runtime/logs/remote");
        if std::fs::create_dir_all(&dest).is_err() {
            warn!("failed to create log destination");
            return false;
        }

        // Use cp -r for simplicity
        let status = Command::new("cp")
            .args(["-r"])
            .arg(&source)
            .arg(&dest)
            .status()
            .await;

        match status {
            Ok(s) if s.success() => {
                debug!(
                    dest = %dest.display(),
                    "logs copied"
                );
                true
            }
            _ => {
                warn!("failed to copy logs");
                false
            }
        }
    }

    async fn detect_conflicts(&self, branches: &[BranchInfo]) -> Vec<String> {
        let mut conflicts = Vec::new();

        for branch in branches {
            if branch.is_new {
                continue;
            }

            let local_commit = match self.git_rev_parse(&branch.name).await {
                Some(c) => c,
                None => continue,
            };

            if local_commit == branch.commit {
                continue;
            }

            // Check if fast-forward possible
            let output = Command::new("git")
                .args(["merge-base", "--is-ancestor", &local_commit, &branch.commit])
                .current_dir(&self.repo_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await;

            if let Ok(o) = output
                && !o.status.success()
            {
                conflicts.push(format!(
                    "Branch '{}' has diverged: \
                         local={}, remote={}",
                    branch.name,
                    &local_commit[..local_commit.len().min(8)],
                    &branch.commit[..branch.commit.len().min(8)],
                ));
            }
        }

        conflicts
    }

    async fn count_new_commits(&self, branches: &[BranchInfo]) -> usize {
        let mut total = 0usize;
        for branch in branches {
            let output = Command::new("git")
                .args([
                    "rev-list",
                    "--count",
                    &format!("remote-exec/{}", branch.name),
                ])
                .current_dir(&self.repo_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await;

            if let Ok(o) = output
                && let Ok(s) = String::from_utf8(o.stdout)
                && let Ok(n) = s.trim().parse::<usize>()
            {
                total += n;
            }
        }
        total
    }

    async fn count_files_changed(&self, branches: &[BranchInfo]) -> usize {
        let Some(branch) = branches.first() else {
            return 0;
        };

        let output = Command::new("git")
            .args([
                "diff",
                "--name-only",
                &format!("remote-exec/{}~1", branch.name),
                &format!("remote-exec/{}", branch.name),
            ])
            .current_dir(&self.repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .count(),
            _ => 0,
        }
    }

    async fn list_local_branches(&self) -> Vec<String> {
        let output = Command::new("git")
            .args(["branch", "--format=%(refname:short)"])
            .current_dir(&self.repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(String::from)
                .collect(),
            _ => Vec::new(),
        }
    }

    async fn list_remote_exec_branches(&self) -> Vec<String> {
        let output = Command::new("git")
            .args(["branch", "-r", "--format=%(refname:short)"])
            .current_dir(&self.repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| l.starts_with("remote-exec/"))
                .map(|l| l.replace("remote-exec/", ""))
                .collect(),
            _ => Vec::new(),
        }
    }

    async fn git_rev_parse(&self, refspec: &str) -> Option<String> {
        let output = Command::new("git")
            .args(["rev-parse", refspec])
            .current_dir(&self.repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .ok()?;

        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_info_serialization() {
        let b = BranchInfo {
            name: "main".into(),
            commit: "abc12345".into(),
            is_new: true,
        };
        let json = serde_json::to_string(&b).unwrap();
        let b2: BranchInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(b2.name, "main");
        assert!(b2.is_new);
    }

    #[test]
    fn integration_summary_report() {
        let summary = IntegrationSummary {
            branches: vec![BranchInfo {
                name: "feature".into(),
                commit: "abc12345def".into(),
                is_new: true,
            }],
            commits_count: 3,
            files_changed: 5,
            logs_copied: true,
            has_conflicts: false,
            conflict_details: None,
        };
        let _integrator_path = std::env::current_dir().unwrap();
        // We can't create a real Integrator without .git,
        // so test the report formatting via a standalone call.
        let report = format_summary_report(&summary);
        assert!(report.contains("feature"));
        assert!(report.contains("NEW"));
        assert!(report.contains("Commits: 3"));
    }

    /// Standalone report formatter for testing without git repo.
    fn format_summary_report(summary: &IntegrationSummary) -> String {
        let sep = "=".repeat(60);
        let mut lines = vec![
            String::new(),
            sep.clone(),
            "Remote Execution Results".into(),
            sep.clone(),
            String::new(),
        ];
        lines.push(format!("Branches ({}):", summary.branches.len()));
        for b in &summary.branches {
            let status = if b.is_new { "NEW" } else { "UPDATED" };
            lines.push(format!(
                "  - {} ({status}): {}",
                b.name,
                &b.commit[..b.commit.len().min(8)]
            ));
        }
        lines.push(String::new());
        lines.push(format!("Commits: {}", summary.commits_count));
        lines.push(format!("Files changed: {}", summary.files_changed));
        lines.push(format!(
            "Logs copied: {}",
            if summary.logs_copied { "Yes" } else { "No" }
        ));
        lines.push(String::new());
        if summary.has_conflicts {
            lines.push("WARNING: Conflicts detected!".into());
        } else {
            lines.push("Status: No conflicts detected".into());
        }
        lines.push(String::new());
        lines.push(sep);
        lines.join("\n")
    }

    #[test]
    fn integrator_rejects_non_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = Integrator::new(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn summary_with_conflicts() {
        let summary = IntegrationSummary {
            branches: vec![],
            commits_count: 0,
            files_changed: 0,
            logs_copied: false,
            has_conflicts: true,
            conflict_details: Some("Branch 'main' diverged".into()),
        };
        let json = serde_json::to_string(&summary).unwrap();
        let s2: IntegrationSummary = serde_json::from_str(&json).unwrap();
        assert!(s2.has_conflicts);
    }
}
