//! Completion signal detection from work summary.
//!
//! Matches Python `amplihack/launcher/completion_signals.py`:
//! - Detect concrete completion markers
//! - Weighted scoring (0.0–1.0)
//! - Threshold-based completion check

use serde::{Deserialize, Serialize};

use crate::work_summary::WorkSummary;

/// Individual signal weight for scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalScore {
    pub name: String,
    pub weight: f64,
    pub detected: bool,
}

/// Concrete completion signals detected from `WorkSummary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionSignals {
    pub all_steps_complete: bool,
    pub pr_created: bool,
    pub ci_passing: bool,
    pub pr_mergeable: bool,
    pub has_commits: bool,
    pub no_uncommitted_changes: bool,
    pub completion_score: f64,
    pub pr_number: Option<u32>,
}

impl CompletionSignals {
    /// Validate that `completion_score` is in range `[0.0, 1.0]`.
    pub fn validate(&self) -> anyhow::Result<()> {
        if !(0.0..=1.0).contains(&self.completion_score) {
            anyhow::bail!(
                "Completion score must be 0.0-1.0, got {}",
                self.completion_score
            );
        }
        Ok(())
    }
}

/// Signal weights — must sum to 1.0.
const WEIGHT_ALL_STEPS: f64 = 0.30;
const WEIGHT_PR_CREATED: f64 = 0.25;
const WEIGHT_CI_PASSING: f64 = 0.20;
const WEIGHT_PR_MERGEABLE: f64 = 0.15;
const WEIGHT_HAS_COMMITS: f64 = 0.05;
const WEIGHT_NO_UNCOMMITTED: f64 = 0.05;

/// Detect completion signals from `WorkSummary`.
pub struct CompletionSignalDetector {
    pub completion_threshold: f64,
}

impl Default for CompletionSignalDetector {
    fn default() -> Self {
        Self {
            completion_threshold: 0.8,
        }
    }
}

impl CompletionSignalDetector {
    pub fn new(completion_threshold: f64) -> Self {
        Self {
            completion_threshold,
        }
    }

    /// Detect all completion signals from `WorkSummary`.
    pub fn detect(&self, summary: &WorkSummary) -> CompletionSignals {
        let todo = &summary.todo_state;
        let git = &summary.git_state;
        let gh = &summary.github_state;

        let all_steps_complete = todo.total > 0 && todo.completed == todo.total;
        let pr_created = gh.pr_number.is_some();
        let ci_passing = gh.ci_status.as_deref() == Some("SUCCESS");
        let pr_mergeable = gh.pr_mergeable == Some(true);
        let has_commits = git.commits_ahead.is_some_and(|c| c > 0);
        let no_uncommitted_changes = !git.has_uncommitted_changes;

        // Partial credit for task completion
        let mut score = 0.0;
        if todo.total > 0 {
            let ratio = f64::from(todo.completed) / f64::from(todo.total);
            score += WEIGHT_ALL_STEPS * ratio;
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
            score += WEIGHT_NO_UNCOMMITTED;
        }

        CompletionSignals {
            all_steps_complete,
            pr_created,
            ci_passing,
            pr_mergeable,
            has_commits,
            no_uncommitted_changes,
            completion_score: score,
            pr_number: gh.pr_number,
        }
    }

    /// Check if signals indicate completion.
    pub fn is_complete(&self, signals: &CompletionSignals) -> bool {
        signals.completion_score >= self.completion_threshold
    }

    /// Human-readable explanation of signals.
    pub fn explain(&self, signals: &CompletionSignals) -> String {
        if self.is_complete(signals) {
            self.explain_complete(signals)
        } else {
            self.explain_incomplete(signals)
        }
    }

    fn explain_complete(&self, signals: &CompletionSignals) -> String {
        let mut lines = vec!["Work appears complete:".to_string()];

        if signals.all_steps_complete {
            lines.push("✓ All tasks completed".into());
        }
        if signals.pr_created {
            let pr_text = signals
                .pr_number
                .map_or("PR".into(), |n| format!("PR #{n}"));
            lines.push(format!("✓ {pr_text} created"));
        }
        if signals.ci_passing {
            lines.push("✓ CI checks passing".into());
        }
        if signals.pr_mergeable {
            lines.push("✓ PR is mergeable".into());
        }
        if signals.has_commits {
            lines.push("✓ Work committed".into());
        }
        if signals.no_uncommitted_changes {
            lines.push("✓ Clean working tree".into());
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

        for item in &missing {
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
    use crate::work_summary::{GitHubState, GitState, TodoState};

    fn make_summary(
        completed: u32,
        total: u32,
        pr: Option<u32>,
        ci: Option<&str>,
        mergeable: Option<bool>,
        commits_ahead: Option<u32>,
        uncommitted: bool,
    ) -> WorkSummary {
        WorkSummary {
            todo_state: TodoState {
                total,
                completed,
                in_progress: 0,
                pending: total - completed,
            },
            git_state: GitState {
                current_branch: Some("feat/test".into()),
                has_uncommitted_changes: uncommitted,
                commits_ahead,
            },
            github_state: GitHubState {
                pr_number: pr,
                pr_state: Some("OPEN".into()),
                ci_status: ci.map(String::from),
                pr_mergeable: mergeable,
            },
        }
    }

    #[test]
    fn perfect_completion_scores_one() {
        let summary = make_summary(5, 5, Some(42), Some("SUCCESS"), Some(true), Some(3), false);
        let detector = CompletionSignalDetector::default();
        let signals = detector.detect(&summary);

        assert!(signals.all_steps_complete);
        assert!(signals.pr_created);
        assert!(signals.ci_passing);
        assert!(signals.pr_mergeable);
        assert!(signals.has_commits);
        assert!(signals.no_uncommitted_changes);
        assert!((signals.completion_score - 1.0).abs() < 0.001);
        assert!(detector.is_complete(&signals));
    }

    #[test]
    fn nothing_done_scores_zero_with_uncommitted() {
        let summary = make_summary(0, 0, None, None, None, None, true);
        let detector = CompletionSignalDetector::default();
        let signals = detector.detect(&summary);
        assert!(!signals.all_steps_complete);
        assert!(!signals.pr_created);
        assert!(signals.completion_score < 0.01);
        assert!(!detector.is_complete(&signals));
    }

    #[test]
    fn partial_tasks_get_partial_credit() {
        let summary = make_summary(3, 6, None, None, None, None, true);
        let detector = CompletionSignalDetector::default();
        let signals = detector.detect(&summary);
        // 3/6 * 0.30 = 0.15
        assert!((signals.completion_score - 0.15).abs() < 0.001);
    }

    #[test]
    fn explain_complete_output() {
        let summary = make_summary(5, 5, Some(42), Some("SUCCESS"), Some(true), Some(3), false);
        let detector = CompletionSignalDetector::default();
        let signals = detector.detect(&summary);
        let explanation = detector.explain(&signals);
        assert!(explanation.contains("Work appears complete:"));
        assert!(explanation.contains("All tasks completed"));
        assert!(explanation.contains("PR #42 created"));
    }

    #[test]
    fn explain_incomplete_output() {
        let summary = make_summary(0, 5, None, None, None, None, true);
        let detector = CompletionSignalDetector::default();
        let signals = detector.detect(&summary);
        let explanation = detector.explain(&signals);
        assert!(explanation.contains("Work incomplete:"));
        assert!(explanation.contains("Tasks pending"));
        assert!(explanation.contains("No PR created"));
    }

    #[test]
    fn custom_threshold() {
        let detector = CompletionSignalDetector::new(0.5);
        let summary = make_summary(5, 5, Some(1), None, None, Some(2), false);
        let signals = detector.detect(&summary);
        // 0.30 (tasks) + 0.25 (PR) + 0.05 (commits) + 0.05 (clean) = 0.65
        assert!(detector.is_complete(&signals));
    }

    #[test]
    fn signals_validate_valid_score() {
        let signals = CompletionSignals {
            all_steps_complete: true,
            pr_created: false,
            ci_passing: false,
            pr_mergeable: false,
            has_commits: false,
            no_uncommitted_changes: false,
            completion_score: 0.5,
            pr_number: None,
        };
        assert!(signals.validate().is_ok());
    }

    #[test]
    fn signals_validate_out_of_range() {
        let signals = CompletionSignals {
            all_steps_complete: false,
            pr_created: false,
            ci_passing: false,
            pr_mergeable: false,
            has_commits: false,
            no_uncommitted_changes: false,
            completion_score: 1.5,
            pr_number: None,
        };
        assert!(signals.validate().is_err());
    }

    #[test]
    fn no_uncommitted_scores_weight() {
        // Only signal: clean working tree
        let summary = make_summary(0, 0, None, None, None, None, false);
        let detector = CompletionSignalDetector::default();
        let signals = detector.detect(&summary);
        assert!((signals.completion_score - WEIGHT_NO_UNCOMMITTED).abs() < 0.001);
    }
}
