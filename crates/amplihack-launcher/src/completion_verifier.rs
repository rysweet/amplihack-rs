//! Completion verification — cross-checks evaluation text against signals.
//!
//! Matches Python `amplihack/launcher/completion_verifier.py`:
//! - Parse completion claims from LLM output
//! - Detect discrepancies between claims and reality
//! - Produce verification reports

use serde::{Deserialize, Serialize};

use crate::completion_signals::CompletionSignals;

/// Verification status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Verified,
    Disputed,
    Incomplete,
    Ambiguous,
}

/// Result of verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub status: VerificationStatus,
    pub verified: bool,
    pub explanation: String,
    pub discrepancies: Vec<String>,
}

/// Verify completion claims against concrete signals.
pub struct CompletionVerifier {
    pub completion_threshold: f64,
}

impl Default for CompletionVerifier {
    fn default() -> Self {
        Self {
            completion_threshold: 0.8,
        }
    }
}

impl CompletionVerifier {
    pub fn new(completion_threshold: f64) -> Self {
        Self {
            completion_threshold,
        }
    }

    /// Verify evaluation result text against signals.
    pub fn verify(
        &self,
        evaluation_result: &str,
        signals: &CompletionSignals,
    ) -> VerificationResult {
        let claimed_complete = Self::parse_completion_claim(evaluation_result);
        let signals_complete = signals.completion_score >= self.completion_threshold;
        let discrepancies =
            self.detect_discrepancies(evaluation_result, signals, claimed_complete);

        match (claimed_complete, signals_complete) {
            (true, true) if discrepancies.is_empty() => VerificationResult {
                status: VerificationStatus::Verified,
                verified: true,
                explanation: "Work is complete - evaluation verified by concrete signals"
                    .to_string(),
                discrepancies: vec![],
            },
            (true, false) => {
                let ci_pending = discrepancies.iter().any(|d| d.contains("CI not passing"));
                let score_close = signals.completion_score >= 0.7;
                let lower = evaluation_result.to_lowercase();
                let eval_ack_ci =
                    lower.contains("waiting") || lower.contains("pending");

                if ci_pending
                    && score_close
                    && signals.pr_created
                    && signals.all_steps_complete
                    && eval_ack_ci
                {
                    return VerificationResult {
                        status: VerificationStatus::Incomplete,
                        verified: false,
                        explanation: "Work mostly complete but CI checks still running"
                            .to_string(),
                        discrepancies,
                    };
                }

                let mut parts = vec![format!(
                    "Evaluation claims complete but score is {:.1}% (threshold {:.1}%)",
                    signals.completion_score * 100.0,
                    self.completion_threshold * 100.0,
                )];
                if !discrepancies.is_empty() {
                    let top: Vec<_> = discrepancies.iter().take(2).cloned().collect();
                    parts.push(format!("Issues: {}", top.join(", ")));
                }

                VerificationResult {
                    status: VerificationStatus::Disputed,
                    verified: false,
                    explanation: parts.join(". "),
                    discrepancies,
                }
            }
            (false, false) => {
                if !discrepancies.is_empty() {
                    let top: Vec<_> = discrepancies.iter().take(2).cloned().collect();
                    VerificationResult {
                        status: VerificationStatus::Disputed,
                        verified: false,
                        explanation: format!(
                            "Evaluation and signals both show incomplete, but details conflict: {}",
                            top.join(", ")
                        ),
                        discrepancies,
                    }
                } else {
                    VerificationResult {
                        status: VerificationStatus::Verified,
                        verified: true,
                        explanation: "Evaluation correctly identifies work as incomplete"
                            .to_string(),
                        discrepancies: vec![],
                    }
                }
            }
            (false, true) => VerificationResult {
                status: VerificationStatus::Disputed,
                verified: false,
                explanation: format!(
                    "Evaluation claims incomplete but score is {:.1}%",
                    signals.completion_score * 100.0
                ),
                discrepancies,
            },
            // claimed_complete && signals_complete but has discrepancies
            _ => VerificationResult {
                status: VerificationStatus::Ambiguous,
                verified: false,
                explanation: "Cannot determine verification status".to_string(),
                discrepancies,
            },
        }
    }

    /// Parse whether the evaluation text claims completion.
    pub fn parse_completion_claim(text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let lower = text.to_lowercase();

        if lower.contains("evaluation: complete") {
            return true;
        }
        if lower.contains("evaluation: incomplete") {
            return false;
        }

        const COMPLETE_PHRASES: &[&str] = &[
            "finished",
            "done",
            "ready to merge",
            "all tasks completed",
            "work is complete",
            "completed successfully",
        ];
        const INCOMPLETE_PHRASES: &[&str] = &[
            "still working",
            "in progress",
            "pending",
            "need to",
            "not done",
            "incomplete",
        ];

        for phrase in COMPLETE_PHRASES {
            if lower.contains(phrase) {
                return true;
            }
        }
        for phrase in INCOMPLETE_PHRASES {
            if lower.contains(phrase) {
                return false;
            }
        }

        false
    }

    fn detect_discrepancies(
        &self,
        evaluation_result: &str,
        signals: &CompletionSignals,
        claimed_complete: bool,
    ) -> Vec<String> {
        let mut discrepancies = Vec::new();
        let lower = evaluation_result.to_lowercase();

        // PR claim vs reality
        let pr_mentioned = lower.contains("pr") || lower.contains("pull request");
        if pr_mentioned && !signals.pr_created {
            discrepancies.push("Evaluation mentions PR but no PR exists".into());
        } else if claimed_complete && !signals.pr_created {
            discrepancies.push("Claims complete but no PR created".into());
        }

        // CI status
        let ci_mentioned =
            lower.contains("ci") || lower.contains("checks") || lower.contains("passing");
        if ci_mentioned && lower.contains("passing") && !signals.ci_passing {
            discrepancies.push("Claims CI passing but CI status is not SUCCESS".into());
        } else if ci_mentioned && lower.contains("failing") && signals.ci_passing {
            discrepancies.push("Claims CI failing but CI status is SUCCESS".into());
        } else if claimed_complete && signals.pr_created && !signals.ci_passing {
            discrepancies.push("Claims complete but CI not passing".into());
        }

        // Tasks complete
        if (lower.contains("all tasks") || lower.contains("tasks completed"))
            && !signals.all_steps_complete
        {
            discrepancies.push(
                "Claims all tasks complete but TodoWrite shows pending tasks".into(),
            );
        } else if claimed_complete && !signals.all_steps_complete {
            discrepancies
                .push("Claims complete but not all TodoWrite tasks finished".into());
        }

        // Uncommitted changes
        if (lower.contains("committed") || lower.contains("pushed"))
            && !signals.no_uncommitted_changes
        {
            discrepancies
                .push("Claims changes committed but uncommitted changes exist".into());
        }

        // Mergeable
        if (lower.contains("ready to merge") || lower.contains("mergeable"))
            && !signals.pr_mergeable
        {
            discrepancies.push(
                "Claims ready to merge but PR has conflicts or is not mergeable".into(),
            );
        }

        discrepancies
    }

    /// Format verification result as a human-readable report.
    pub fn format_report(result: &VerificationResult) -> String {
        let status_label = match result.status {
            VerificationStatus::Verified => "VERIFIED",
            VerificationStatus::Disputed => "DISPUTED",
            VerificationStatus::Incomplete => "INCOMPLETE",
            VerificationStatus::Ambiguous => "AMBIGUOUS",
        };
        let mut lines = vec![format!("Verification: {status_label}")];

        if result.verified {
            lines.push(format!("✓ {}", result.explanation));
        } else {
            lines.push(format!("✗ {}", result.explanation));
        }

        if !result.discrepancies.is_empty() {
            lines.push("\nDiscrepancies found:".into());
            for d in &result.discrepancies {
                lines.push(format!("  - {d}"));
            }
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn make_signals(
        all_steps: bool,
        pr_created: bool,
        ci: bool,
        mergeable: bool,
        commits: bool,
        clean: bool,
        score: f64,
        pr_num: Option<u32>,
    ) -> CompletionSignals {
        CompletionSignals {
            all_steps_complete: all_steps,
            pr_created,
            ci_passing: ci,
            pr_mergeable: mergeable,
            has_commits: commits,
            no_uncommitted_changes: clean,
            completion_score: score,
            pr_number: pr_num,
        }
    }

    #[test]
    fn parse_claim_explicit_complete() {
        assert!(CompletionVerifier::parse_completion_claim(
            "Evaluation: complete"
        ));
    }

    #[test]
    fn parse_claim_explicit_incomplete() {
        assert!(!CompletionVerifier::parse_completion_claim(
            "Evaluation: incomplete"
        ));
    }

    #[test]
    fn parse_claim_implicit_done() {
        assert!(CompletionVerifier::parse_completion_claim(
            "All tasks are done."
        ));
    }

    #[test]
    fn parse_claim_implicit_pending() {
        assert!(!CompletionVerifier::parse_completion_claim(
            "There are still tasks pending."
        ));
    }

    #[test]
    fn parse_claim_empty() {
        assert!(!CompletionVerifier::parse_completion_claim(""));
    }

    #[test]
    fn parse_claim_ambiguous_defaults_false() {
        assert!(!CompletionVerifier::parse_completion_claim(
            "Some random text about nothing"
        ));
    }

    #[test]
    fn verified_complete() {
        let verifier = CompletionVerifier::default();
        let signals = make_signals(true, true, true, true, true, true, 1.0, Some(42));
        let result = verifier.verify("Evaluation: complete. All tasks completed.", &signals);
        assert_eq!(result.status, VerificationStatus::Verified);
        assert!(result.verified);
    }

    #[test]
    fn disputed_false_claim() {
        let verifier = CompletionVerifier::default();
        let signals = make_signals(false, false, false, false, false, true, 0.05, None);
        let result = verifier.verify("Everything is done!", &signals);
        assert_eq!(result.status, VerificationStatus::Disputed);
        assert!(!result.verified);
    }

    #[test]
    fn verified_correctly_incomplete() {
        let verifier = CompletionVerifier::default();
        let signals = make_signals(false, false, false, false, false, true, 0.05, None);
        // Note: "still working" doesn't contain "pr" substring, avoiding false PR detection
        let result = verifier.verify("Still working on the tasks.", &signals);
        assert_eq!(result.status, VerificationStatus::Verified);
        assert!(result.verified);
    }

    #[test]
    fn disputed_overly_conservative() {
        let verifier = CompletionVerifier::default();
        let signals = make_signals(true, true, true, true, true, true, 1.0, Some(1));
        let result = verifier.verify("Still working on it, need to do more", &signals);
        assert_eq!(result.status, VerificationStatus::Disputed);
    }

    #[test]
    fn ci_pending_incomplete_status() {
        let verifier = CompletionVerifier::default();
        // All steps done, PR created, but CI not passing — score just under threshold
        let signals = make_signals(true, true, false, false, true, true, 0.75, Some(5));
        let result =
            verifier.verify("All tasks completed. Evaluation: complete. CI still pending.", &signals);
        // Score 0.75 >= 0.7, CI pending acknowledged, pr_created, all_steps_complete
        assert_eq!(result.status, VerificationStatus::Incomplete);
    }

    #[test]
    fn format_report_verified() {
        let result = VerificationResult {
            status: VerificationStatus::Verified,
            verified: true,
            explanation: "Work is complete".into(),
            discrepancies: vec![],
        };
        let report = CompletionVerifier::format_report(&result);
        assert!(report.contains("VERIFIED"));
        assert!(report.contains("✓"));
    }

    #[test]
    fn format_report_disputed_with_discrepancies() {
        let result = VerificationResult {
            status: VerificationStatus::Disputed,
            verified: false,
            explanation: "Score too low".into(),
            discrepancies: vec!["No PR created".into(), "Tasks pending".into()],
        };
        let report = CompletionVerifier::format_report(&result);
        assert!(report.contains("DISPUTED"));
        assert!(report.contains("✗"));
        assert!(report.contains("Discrepancies found:"));
        assert!(report.contains("No PR created"));
    }

    #[test]
    fn discrepancy_pr_mentioned_but_absent() {
        let verifier = CompletionVerifier::default();
        let signals = make_signals(false, false, false, false, false, true, 0.05, None);
        let result = verifier.verify("The PR is ready for review", &signals);
        assert!(
            result
                .discrepancies
                .iter()
                .any(|d| d.contains("mentions PR but no PR exists"))
        );
    }

    #[test]
    fn discrepancy_ci_claim_mismatch() {
        let verifier = CompletionVerifier::default();
        let signals = make_signals(true, true, false, true, true, true, 0.8, Some(1));
        let result = verifier.verify("CI checks are passing", &signals);
        assert!(
            result
                .discrepancies
                .iter()
                .any(|d| d.contains("CI passing but CI status is not SUCCESS"))
        );
    }
}
