//! Auto-mode work summary data structures.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TodoState {
    pub total: usize,
    pub completed: usize,
    pub in_progress: usize,
    pub pending: usize,
}

impl TodoState {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.completed + self.in_progress + self.pending != self.total {
            anyhow::bail!(
                "todo counts do not sum to total: {} + {} + {} != {}",
                self.completed,
                self.in_progress,
                self.pending,
                self.total
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitState {
    pub current_branch: Option<String>,
    pub has_uncommitted_changes: bool,
    pub commits_ahead: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitHubState {
    pub pr_number: Option<u64>,
    pub pr_state: Option<String>,
    pub ci_status: Option<String>,
    pub pr_mergeable: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkSummary {
    pub todo_state: TodoState,
    pub git_state: GitState,
    pub github_state: GitHubState,
}

impl WorkSummary {
    pub fn format_for_prompt(&self) -> String {
        let mut lines = vec!["Work Summary:".to_string()];

        let todo = &self.todo_state;
        if todo.total > 0 {
            lines.push(format!(
                "- Tasks: {}/{} tasks completed, {} in progress, {} pending",
                todo.completed, todo.total, todo.in_progress, todo.pending
            ));
        } else {
            lines.push("- Tasks: No TodoWrite entries".to_string());
        }

        let git = &self.git_state;
        if let Some(branch) = &git.current_branch {
            lines.push(format!("- Branch: {branch}"));
            if let Some(commits_ahead) = git.commits_ahead {
                lines.push(format!("- Commits ahead: {commits_ahead}"));
            }
            lines.push(format!(
                "- Uncommitted changes: {}",
                if git.has_uncommitted_changes {
                    "Yes"
                } else {
                    "No"
                }
            ));
        } else {
            lines.push("- Git: Not in repository".to_string());
        }

        let github = &self.github_state;
        if let Some(pr_number) = github.pr_number {
            let pr_state = github.pr_state.as_deref().unwrap_or("UNKNOWN");
            lines.push(format!("- PR: #{pr_number} ({pr_state})"));
            if let Some(ci_status) = github.ci_status.as_deref() {
                let status_text = if ci_status == "SUCCESS" {
                    "passing"
                } else {
                    ci_status
                };
                lines.push(format!("- CI Status: {status_text}"));
            }
            if let Some(pr_mergeable) = github.pr_mergeable {
                let mergeable_text = if pr_mergeable {
                    "yes"
                } else {
                    "no (conflicts)"
                };
                lines.push(format!("- Mergeable: {mergeable_text}"));
            }
        } else {
            lines.push("- PR: not created".to_string());
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_state_validate_rejects_bad_totals() {
        let todo = TodoState {
            total: 3,
            completed: 1,
            in_progress: 1,
            pending: 0,
        };

        assert!(todo.validate().is_err());
    }

    #[test]
    fn format_for_prompt_includes_key_sections() {
        let summary = WorkSummary {
            todo_state: TodoState {
                total: 3,
                completed: 1,
                in_progress: 1,
                pending: 1,
            },
            git_state: GitState {
                current_branch: Some("feature/parity".to_string()),
                has_uncommitted_changes: false,
                commits_ahead: Some(2),
            },
            github_state: GitHubState {
                pr_number: Some(77),
                pr_state: Some("OPEN".to_string()),
                ci_status: Some("SUCCESS".to_string()),
                pr_mergeable: Some(true),
            },
        };

        let formatted = summary.format_for_prompt();

        assert!(formatted.contains("- Tasks: 1/3 tasks completed, 1 in progress, 1 pending"));
        assert!(formatted.contains("- Branch: feature/parity"));
        assert!(formatted.contains("- Commits ahead: 2"));
        assert!(formatted.contains("- CI Status: passing"));
        assert!(formatted.contains("- Mergeable: yes"));
    }
}
