//! Auto-mode completion signal scoring.

use crate::auto_mode_work_summary::WorkSummary;

const WEIGHT_ALL_STEPS_COMPLETE: f64 = 0.30;
const WEIGHT_PR_CREATED: f64 = 0.25;
const WEIGHT_CI_PASSING: f64 = 0.20;
const WEIGHT_PR_MERGEABLE: f64 = 0.15;
const WEIGHT_HAS_COMMITS: f64 = 0.05;
const WEIGHT_NO_UNCOMMITTED_CHANGES: f64 = 0.05;

#[derive(Clone, Debug, PartialEq)]
pub struct CompletionSignals {
    pub all_steps_complete: bool,
    pub pr_created: bool,
    pub ci_passing: bool,
    pub pr_mergeable: bool,
    pub has_commits: bool,
    pub no_uncommitted_changes: bool,
    pub completion_score: f64,
    pub pr_number: Option<u64>,
}

impl CompletionSignals {
    fn validate(&self) {
        assert!(
            (0.0..=1.0).contains(&self.completion_score),
            "completion score must be 0.0-1.0, got {}",
            self.completion_score
        );
    }
}

#[derive(Clone, Debug)]
pub struct CompletionSignalDetector {
    completion_threshold: f64,
}

impl Default for CompletionSignalDetector {
    fn default() -> Self {
        Self::new(0.8)
    }
}

impl CompletionSignalDetector {
    pub fn new(completion_threshold: f64) -> Self {
        Self {
            completion_threshold,
        }
    }

    pub fn detect(&self, summary: &WorkSummary) -> CompletionSignals {
        let all_steps_complete = self.detect_all_steps_complete(summary);
        let pr_created = self.detect_pr_created(summary);
        let ci_passing = self.detect_ci_passing(summary);
        let pr_mergeable = self.detect_pr_mergeable(summary);
        let has_commits = self.detect_has_commits(summary);
        let no_uncommitted_changes = self.detect_no_uncommitted_changes(summary);

        let mut score = 0.0;
        let todo = &summary.todo_state;
        if todo.total > 0 {
            let completion_ratio = todo.completed as f64 / todo.total as f64;
            score += WEIGHT_ALL_STEPS_COMPLETE * completion_ratio;
        }
        if pr_created {
            score += WEIGHT_PR_CREATED;
        }
        if ci_passing {
            score += WEIGHT_CI_PASSING;
        }
        if pr_mergeable {
            score += WEIGHT_PR_MERGEABLE;
        }
        if has_commits {
            score += WEIGHT_HAS_COMMITS;
        }
        if no_uncommitted_changes {
            score += WEIGHT_NO_UNCOMMITTED_CHANGES;
        }

        let signals = CompletionSignals {
            all_steps_complete,
            pr_created,
            ci_passing,
            pr_mergeable,
            has_commits,
            no_uncommitted_changes,
            completion_score: score,
            pr_number: summary.github_state.pr_number,
        };
        signals.validate();
        signals
    }

    pub fn is_complete(&self, signals: &CompletionSignals) -> bool {
        signals.completion_score >= self.completion_threshold
    }

    pub fn explain(&self, signals: &CompletionSignals) -> String {
        if self.is_complete(signals) {
            self.explain_complete(signals)
        } else {
            self.explain_incomplete(signals)
        }
    }

    fn detect_all_steps_complete(&self, summary: &WorkSummary) -> bool {
        let todo = &summary.todo_state;
        todo.total > 0 && todo.completed == todo.total
    }

    fn detect_pr_created(&self, summary: &WorkSummary) -> bool {
        summary.github_state.pr_number.is_some()
    }

    fn detect_ci_passing(&self, summary: &WorkSummary) -> bool {
        summary.github_state.ci_status.as_deref() == Some("SUCCESS")
    }

    fn detect_pr_mergeable(&self, summary: &WorkSummary) -> bool {
        summary.github_state.pr_mergeable == Some(true)
    }

    fn detect_has_commits(&self, summary: &WorkSummary) -> bool {
        summary
            .git_state
            .commits_ahead
            .is_some_and(|commits_ahead| commits_ahead > 0)
    }

    fn detect_no_uncommitted_changes(&self, summary: &WorkSummary) -> bool {
        !summary.git_state.has_uncommitted_changes
    }

    fn explain_complete(&self, signals: &CompletionSignals) -> String {
        let mut lines = vec!["Work appears complete:".to_string()];

        if signals.all_steps_complete {
            lines.push("✓ All tasks completed".to_string());
        }
        if signals.pr_created {
            let pr_text = signals
                .pr_number
                .map(|pr_number| format!("PR #{pr_number}"))
                .unwrap_or_else(|| "PR".to_string());
            lines.push(format!("✓ {pr_text} created"));
        }
        if signals.ci_passing {
            lines.push("✓ CI checks passing".to_string());
        }
        if signals.pr_mergeable {
            lines.push("✓ PR is mergeable".to_string());
        }
        if signals.has_commits {
            lines.push("✓ Work committed".to_string());
        }
        if signals.no_uncommitted_changes {
            lines.push("✓ Clean working tree".to_string());
        }

        lines.push(format!(
            "\nCompletion score: {:.1}%",
            signals.completion_score * 100.0
        ));
        lines.join("\n")
    }

    fn explain_incomplete(&self, signals: &CompletionSignals) -> String {
        let mut lines = vec!["Work incomplete:".to_string()];
        let mut missing = Vec::new();

        if !signals.all_steps_complete {
            missing.push("Tasks pending");
        }
        if !signals.pr_created {
            missing.push("No PR created");
        }
        if !signals.ci_passing && signals.pr_created {
            missing.push("CI not passing");
        }
        if !signals.pr_mergeable && signals.pr_created {
            missing.push("PR has conflicts");
        }
        if !signals.has_commits {
            missing.push("No commits");
        }
        if !signals.no_uncommitted_changes {
            missing.push("Uncommitted changes exist");
        }

        for item in missing {
            lines.push(format!("✗ {item}"));
        }
        lines.push(format!(
            "\nCompletion score: {:.1}%",
            signals.completion_score * 100.0
        ));
        lines.push(format!(
            "(Threshold: {:.1}%)",
            self.completion_threshold * 100.0
        ));
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_mode_work_summary::{GitHubState, GitState, TodoState, WorkSummary};

    fn sample_summary() -> WorkSummary {
        WorkSummary {
            todo_state: TodoState {
                total: 4,
                completed: 2,
                in_progress: 1,
                pending: 1,
            },
            git_state: GitState {
                current_branch: Some("feature/parity".to_string()),
                has_uncommitted_changes: true,
                commits_ahead: Some(3),
            },
            github_state: GitHubState {
                pr_number: Some(77),
                pr_state: Some("OPEN".to_string()),
                ci_status: Some("PENDING".to_string()),
                pr_mergeable: Some(false),
            },
        }
    }

    #[test]
    fn detect_uses_partial_credit_for_todo_completion() {
        let detector = CompletionSignalDetector::default();
        let signals = detector.detect(&sample_summary());

        assert_eq!(signals.completion_score, 0.45);
        assert!(signals.pr_created);
        assert!(signals.has_commits);
        assert!(!signals.ci_passing);
    }

    #[test]
    fn explain_complete_lists_positive_signals() {
        let detector = CompletionSignalDetector::default();
        let summary = WorkSummary {
            todo_state: TodoState {
                total: 2,
                completed: 2,
                in_progress: 0,
                pending: 0,
            },
            git_state: GitState {
                current_branch: Some("feature/parity".to_string()),
                has_uncommitted_changes: false,
                commits_ahead: Some(1),
            },
            github_state: GitHubState {
                pr_number: Some(77),
                pr_state: Some("OPEN".to_string()),
                ci_status: Some("SUCCESS".to_string()),
                pr_mergeable: Some(true),
            },
        };

        let signals = detector.detect(&summary);
        let explanation = detector.explain(&signals);

        assert!(detector.is_complete(&signals));
        assert!(explanation.contains("✓ All tasks completed"));
        assert!(explanation.contains("✓ PR #77 created"));
        assert!(explanation.contains("Completion score: 100.0%"));
    }

    #[test]
    fn explain_incomplete_lists_missing_signals() {
        let detector = CompletionSignalDetector::default();
        let signals = detector.detect(&sample_summary());
        let explanation = detector.explain(&signals);

        assert!(explanation.contains("✗ Tasks pending"));
        assert!(explanation.contains("✗ CI not passing"));
        assert!(explanation.contains("✗ PR has conflicts"));
        assert!(explanation.contains("✗ Uncommitted changes exist"));
        assert!(explanation.contains("(Threshold: 80.0%)"));
    }
}
