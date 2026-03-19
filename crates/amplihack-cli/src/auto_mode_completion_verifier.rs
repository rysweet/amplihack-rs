//! Auto-mode completion verification against concrete signals.

use crate::auto_mode_completion_signals::CompletionSignals;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerificationStatus {
    Verified,
    Disputed,
    Incomplete,
    Ambiguous,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationResult {
    pub status: VerificationStatus,
    pub verified: bool,
    pub explanation: String,
    pub discrepancies: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CompletionVerifier {
    completion_threshold: f64,
}

impl Default for CompletionVerifier {
    fn default() -> Self {
        Self::new(0.8)
    }
}

impl CompletionVerifier {
    pub fn new(completion_threshold: f64) -> Self {
        Self {
            completion_threshold,
        }
    }

    pub fn verify(
        &self,
        evaluation_result: &str,
        signals: &CompletionSignals,
    ) -> VerificationResult {
        let claimed_complete = self.parse_completion_claim(evaluation_result);
        let signals_complete = signals.completion_score >= self.completion_threshold;
        let discrepancies = self.detect_discrepancies(evaluation_result, signals, claimed_complete);

        if claimed_complete && signals_complete && discrepancies.is_empty() {
            return VerificationResult {
                status: VerificationStatus::Verified,
                verified: true,
                explanation: "Work is complete - evaluation verified by concrete signals"
                    .to_string(),
                discrepancies: Vec::new(),
            };
        }

        if claimed_complete && !signals_complete {
            let ci_pending = discrepancies
                .iter()
                .any(|discrepancy| discrepancy.contains("CI not passing"));
            let score_close = signals.completion_score >= 0.7;
            let text_lower = evaluation_result.to_ascii_lowercase();
            let eval_acknowledges_ci =
                text_lower.contains("waiting") || text_lower.contains("pending");

            if ci_pending
                && score_close
                && signals.pr_created
                && signals.all_steps_complete
                && eval_acknowledges_ci
            {
                return VerificationResult {
                    status: VerificationStatus::Incomplete,
                    verified: false,
                    explanation: "Work mostly complete but CI checks still running".to_string(),
                    discrepancies,
                };
            }

            let mut explanation = format!(
                "Evaluation claims complete but score is {:.1}% (threshold {:.1}%)",
                signals.completion_score * 100.0,
                self.completion_threshold * 100.0
            );
            if !discrepancies.is_empty() {
                explanation.push_str(". Issues: ");
                explanation.push_str(
                    &discrepancies
                        .iter()
                        .take(2)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            return VerificationResult {
                status: VerificationStatus::Disputed,
                verified: false,
                explanation,
                discrepancies,
            };
        }

        if !claimed_complete && !signals_complete {
            if !discrepancies.is_empty() {
                return VerificationResult {
                    status: VerificationStatus::Disputed,
                    verified: false,
                    explanation: format!(
                        "Evaluation and signals both show incomplete, but details conflict: {}",
                        discrepancies
                            .iter()
                            .take(2)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    discrepancies,
                };
            }
            return VerificationResult {
                status: VerificationStatus::Verified,
                verified: true,
                explanation: "Evaluation correctly identifies work as incomplete".to_string(),
                discrepancies: Vec::new(),
            };
        }

        if !claimed_complete && signals_complete {
            return VerificationResult {
                status: VerificationStatus::Disputed,
                verified: false,
                explanation: format!(
                    "Evaluation claims incomplete but score is {:.1}%",
                    signals.completion_score * 100.0
                ),
                discrepancies,
            };
        }

        VerificationResult {
            status: VerificationStatus::Ambiguous,
            verified: false,
            explanation: "Cannot determine verification status".to_string(),
            discrepancies,
        }
    }

    fn parse_completion_claim(&self, evaluation_result: &str) -> bool {
        if evaluation_result.is_empty() {
            return false;
        }

        let text_lower = evaluation_result.to_ascii_lowercase();
        if text_lower.contains("evaluation: complete") {
            return true;
        }
        if text_lower.contains("evaluation: incomplete") {
            return false;
        }

        for phrase in [
            "finished",
            "done",
            "ready to merge",
            "all tasks completed",
            "work is complete",
            "completed successfully",
        ] {
            if text_lower.contains(phrase) {
                return true;
            }
        }

        for phrase in [
            "still working",
            "in progress",
            "pending",
            "need to",
            "not done",
            "incomplete",
        ] {
            if text_lower.contains(phrase) {
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
        let text_lower = evaluation_result.to_ascii_lowercase();

        let pr_mentioned = text_lower.contains("pr") || text_lower.contains("pull request");
        if pr_mentioned && !signals.pr_created {
            discrepancies.push("Evaluation mentions PR but no PR exists".to_string());
        } else if claimed_complete && !signals.pr_created {
            discrepancies.push("Claims complete but no PR created".to_string());
        }

        let ci_mentioned = text_lower.contains("ci")
            || text_lower.contains("checks")
            || text_lower.contains("passing");
        if ci_mentioned && text_lower.contains("passing") && !signals.ci_passing {
            discrepancies.push("Claims CI passing but CI status is not SUCCESS".to_string());
        } else if ci_mentioned && text_lower.contains("failing") && signals.ci_passing {
            discrepancies.push("Claims CI failing but CI status is SUCCESS".to_string());
        } else if claimed_complete && signals.pr_created && !signals.ci_passing {
            discrepancies.push("Claims complete but CI not passing".to_string());
        }

        if (text_lower.contains("all tasks") || text_lower.contains("tasks completed"))
            && !signals.all_steps_complete
        {
            discrepancies
                .push("Claims all tasks complete but TodoWrite shows pending tasks".to_string());
        } else if claimed_complete && !signals.all_steps_complete {
            discrepancies.push("Claims complete but not all TodoWrite tasks finished".to_string());
        }

        if (text_lower.contains("committed") || text_lower.contains("pushed"))
            && !signals.no_uncommitted_changes
        {
            discrepancies
                .push("Claims changes committed but uncommitted changes exist".to_string());
        }

        if (text_lower.contains("ready to merge") || text_lower.contains("mergeable"))
            && !signals.pr_mergeable
        {
            discrepancies
                .push("Claims ready to merge but PR has conflicts or is not mergeable".to_string());
        }

        discrepancies
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_mode_completion_signals::CompletionSignals;

    fn complete_signals() -> CompletionSignals {
        CompletionSignals {
            all_steps_complete: true,
            pr_created: true,
            ci_passing: true,
            pr_mergeable: true,
            has_commits: true,
            no_uncommitted_changes: true,
            completion_score: 1.0,
            pr_number: Some(77),
        }
    }

    fn ci_pending_signals() -> CompletionSignals {
        CompletionSignals {
            all_steps_complete: true,
            pr_created: true,
            ci_passing: false,
            pr_mergeable: true,
            has_commits: true,
            no_uncommitted_changes: false,
            completion_score: 0.75,
            pr_number: Some(77),
        }
    }

    #[test]
    fn verify_marks_matching_complete_claim_as_verified() {
        let verifier = CompletionVerifier::default();
        let result = verifier.verify("Evaluation: complete. Ready to merge.", &complete_signals());

        assert_eq!(result.status, VerificationStatus::Verified);
        assert!(result.verified);
        assert!(result.discrepancies.is_empty());
    }

    #[test]
    fn verify_marks_ci_pending_complete_claim_as_incomplete() {
        let verifier = CompletionVerifier::default();
        let result = verifier.verify(
            "Evaluation: complete. Work is done, but CI is still pending and we are waiting.",
            &ci_pending_signals(),
        );

        assert_eq!(result.status, VerificationStatus::Incomplete);
        assert!(!result.verified);
        assert!(result.explanation.contains("CI checks still running"));
    }

    #[test]
    fn verify_disputes_conservative_claim_when_signals_are_complete() {
        let verifier = CompletionVerifier::default();
        let result = verifier.verify(
            "Evaluation: incomplete. Still working.",
            &complete_signals(),
        );

        assert_eq!(result.status, VerificationStatus::Disputed);
        assert!(!result.verified);
        assert!(
            result
                .explanation
                .contains("claims incomplete but score is 100.0%")
        );
    }
}
