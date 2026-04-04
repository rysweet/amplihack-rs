//! Work summary generation from session state.
//!
//! Matches Python `amplihack/launcher/work_summary.py`:
//! - TodoState from message capture
//! - Git repository state
//! - GitHub PR state
//! - Format for prompt injection

use serde::{Deserialize, Serialize};
use std::process::Command;

/// TodoWrite task state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoState {
    pub total: u32,
    pub completed: u32,
    pub in_progress: u32,
    pub pending: u32,
}

impl TodoState {
    /// Create a new `TodoState`, validating that counts sum to total.
    pub fn new(total: u32, completed: u32, in_progress: u32, pending: u32) -> anyhow::Result<Self> {
        let sum = completed + in_progress + pending;
        if sum != total {
            anyhow::bail!(
                "Todo counts don't sum to total: {completed} + {in_progress} + {pending} != {total}"
            );
        }
        Ok(Self {
            total,
            completed,
            in_progress,
            pending,
        })
    }

    /// Create an empty `TodoState`.
    pub fn empty() -> Self {
        Self {
            total: 0,
            completed: 0,
            in_progress: 0,
            pending: 0,
        }
    }
}

/// Git repository state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitState {
    pub current_branch: Option<String>,
    pub has_uncommitted_changes: bool,
    pub commits_ahead: Option<u32>,
}

/// GitHub PR state (optional, requires `gh` CLI).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitHubState {
    pub pr_number: Option<u32>,
    pub pr_state: Option<String>,
    pub ci_status: Option<String>,
    pub pr_mergeable: Option<bool>,
}

impl GitHubState {
    pub fn empty() -> Self {
        Self {
            pr_number: None,
            pr_state: None,
            ci_status: None,
            pr_mergeable: None,
        }
    }
}

/// Complete work summary from all sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkSummary {
    pub todo_state: TodoState,
    pub git_state: GitState,
    pub github_state: GitHubState,
}

/// Trait for extracting todo state from message capture.
pub trait TodoExtractor {
    /// Extract todo items from the captured messages.
    fn extract_todos(&self) -> Vec<serde_json::Value>;
}

/// Generate `WorkSummary` from session state and external tools.
pub struct WorkSummaryGenerator {
    cache: Option<WorkSummary>,
}

impl Default for WorkSummaryGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkSummaryGenerator {
    pub fn new() -> Self {
        Self { cache: None }
    }

    /// Generate complete `WorkSummary`.
    pub fn generate(&mut self, extractor: &dyn TodoExtractor) -> WorkSummary {
        if let Some(ref cached) = self.cache {
            return cached.clone();
        }

        let todo_state = Self::extract_todo_state(extractor);
        let git_state = Self::extract_git_state();
        let github_state = git_state
            .current_branch
            .as_deref()
            .map_or_else(GitHubState::empty, Self::extract_github_state);

        let summary = WorkSummary {
            todo_state,
            git_state,
            github_state,
        };
        self.cache = Some(summary.clone());
        summary
    }

    fn extract_todo_state(extractor: &dyn TodoExtractor) -> TodoState {
        let todos = extractor.extract_todos();
        if todos.is_empty() {
            return TodoState::empty();
        }

        let mut completed = 0u32;
        let mut in_progress = 0u32;
        let mut pending = 0u32;

        for todo in &todos {
            match todo.get("status").and_then(|s| s.as_str()) {
                Some("completed") => completed += 1,
                Some("in_progress") => in_progress += 1,
                Some("pending") => pending += 1,
                _ => {}
            }
        }

        let total = completed + in_progress + pending;
        TodoState {
            total,
            completed,
            in_progress,
            pending,
        }
    }

    fn extract_git_state() -> GitState {
        let branch = run_git(&["rev-parse", "--abbrev-ref", "HEAD"]);
        let current_branch = branch.as_deref().map(str::trim).map(String::from);

        let status_out = run_git(&["status", "--porcelain"]);
        let has_uncommitted_changes = status_out
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

        let commits_ahead = run_git(&["rev-list", "--count", "@{u}..HEAD"])
            .and_then(|s| s.trim().parse::<u32>().ok());

        GitState {
            current_branch,
            has_uncommitted_changes,
            commits_ahead,
        }
    }

    fn extract_github_state(branch: &str) -> GitHubState {
        let output = run_cmd(
            "gh",
            &[
                "pr",
                "list",
                "--head",
                branch,
                "--json",
                "number,state,statusCheckRollup,mergeable",
            ],
        );

        let prs: Vec<serde_json::Value> = match output.and_then(|s| serde_json::from_str(&s).ok()) {
            Some(v) => v,
            None => return GitHubState::empty(),
        };

        let pr = match prs.first() {
            Some(p) => p,
            None => return GitHubState::empty(),
        };

        let pr_number = pr.get("number").and_then(|v| v.as_u64()).map(|n| n as u32);
        let pr_state = pr.get("state").and_then(|v| v.as_str()).map(String::from);

        let ci_status = pr
            .get("statusCheckRollup")
            .and_then(|v| v.as_array())
            .and_then(|checks| {
                checks.iter().find_map(|check| {
                    let status = check.get("status")?.as_str()?;
                    if status == "IN_PROGRESS" {
                        Some("PENDING".to_string())
                    } else if status == "COMPLETED" {
                        check
                            .get("conclusion")
                            .and_then(|c| c.as_str())
                            .map(String::from)
                    } else {
                        None
                    }
                })
            });

        let pr_mergeable = pr
            .get("mergeable")
            .and_then(|v| v.as_str())
            .map(|m| match m {
                "MERGEABLE" => true,
                "CONFLICTING" => false,
                _ => false,
            });

        GitHubState {
            pr_number,
            pr_state,
            ci_status,
            pr_mergeable,
        }
    }

    /// Format `WorkSummary` for LLM prompt injection.
    pub fn format_for_prompt(summary: &WorkSummary) -> String {
        let mut lines = vec!["Work Summary:".to_string()];

        let todo = &summary.todo_state;
        if todo.total > 0 {
            lines.push(format!(
                "- Tasks: {}/{} tasks completed, {} in progress, {} pending",
                todo.completed, todo.total, todo.in_progress, todo.pending,
            ));
        } else {
            lines.push("- Tasks: No TodoWrite entries".into());
        }

        let git = &summary.git_state;
        if let Some(ref branch) = git.current_branch {
            lines.push(format!("- Branch: {branch}"));
            if let Some(ahead) = git.commits_ahead {
                lines.push(format!("- Commits ahead: {ahead}"));
            }
            if git.has_uncommitted_changes {
                lines.push("- Uncommitted changes: Yes".into());
            } else {
                lines.push("- Uncommitted changes: No".into());
            }
        } else {
            lines.push("- Git: Not in repository".into());
        }

        let gh = &summary.github_state;
        if let Some(pr_num) = gh.pr_number {
            let state = gh.pr_state.as_deref().unwrap_or("UNKNOWN");
            lines.push(format!("- PR: #{pr_num} ({state})"));
            if let Some(ref ci) = gh.ci_status {
                let text = if ci == "SUCCESS" { "passing" } else { ci };
                lines.push(format!("- CI Status: {text}"));
            }
            if let Some(mergeable) = gh.pr_mergeable {
                let text = if mergeable { "yes" } else { "no (conflicts)" };
                lines.push(format!("- Mergeable: {text}"));
            }
        } else {
            lines.push("- PR: not created".into());
        }

        lines.join("\n")
    }
}

fn run_git(args: &[&str]) -> Option<String> {
    run_cmd("git", args)
}

fn run_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

#[cfg(test)]
#[path = "work_summary_tests.rs"]
mod work_summary_tests;
