//! Self-improvement loop: error analysis, patch proposals, and reviewer voting.

use crate::error::EvalError;
use crate::models::SelfImproveConfig;
use crate::self_improve_helpers::classify_failure;

/// Represents an analyzed test failure.
#[derive(Debug, Clone)]
pub struct FailureAnalysis {
    pub test_id: String,
    pub error_category: String,
    pub root_cause: String,
    pub suggested_fix: String,
}

/// Represents a proposed code patch.
#[derive(Debug, Clone)]
pub struct Patch {
    pub id: String,
    pub description: String,
    pub diff: String,
    pub confidence: f64,
}

/// Reviewer vote on a patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vote {
    Approve,
    Reject,
    Abstain,
}

/// Result of a reviewer voting round.
#[derive(Debug, Clone)]
pub struct VotingResult {
    pub patch_id: String,
    pub votes: Vec<Vote>,
}

impl VotingResult {
    /// Whether the patch was approved (delegates to `has_majority()`).
    pub fn approved(&self) -> bool {
        self.has_majority()
    }

    /// Count approvals.
    pub fn approval_count(&self) -> usize {
        self.votes.iter().filter(|v| **v == Vote::Approve).count()
    }

    /// Count rejections.
    pub fn rejection_count(&self) -> usize {
        self.votes.iter().filter(|v| **v == Vote::Reject).count()
    }

    /// Check if majority approved (among non-abstaining voters).
    pub fn has_majority(&self) -> bool {
        let non_abstain: usize = self.votes.iter().filter(|v| **v != Vote::Abstain).count();
        if non_abstain == 0 {
            return false;
        }
        self.approval_count() > non_abstain / 2
    }
}

/// Result of a self-improvement iteration.
#[derive(Debug, Clone)]
pub struct IterationResult {
    pub iteration: u32,
    pub score_before: f64,
    pub score_after: f64,
    pub patches_proposed: usize,
    pub patches_applied: usize,
    pub improved: bool,
}

/// Analyzes test failures to identify root causes.
pub struct ErrorAnalyzer;

impl Default for ErrorAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl ErrorAnalyzer {
    pub fn new() -> Self {
        Self
    }

    /// Analyze a set of test failures.
    ///
    /// Each failure is `(test_id, error_message)`. Classifies into
    /// the failure taxonomy and suggests fixes.
    pub fn analyze(
        &self,
        failures: &[(String, String)],
    ) -> Result<Vec<FailureAnalysis>, EvalError> {
        let mut analyses = Vec::with_capacity(failures.len());

        for (test_id, error_msg) in failures {
            let lower = error_msg.to_lowercase();
            let (category, root_cause) = classify_failure(&lower);

            let suggested_fix = match category {
                "retrieval_insufficient" => {
                    "Improve retrieval coverage: expand search terms or lower threshold"
                }
                "temporal_ordering_wrong" => {
                    "Fix temporal reasoning: ensure chronological ordering in memory"
                }
                "intent_misclassification" => {
                    "Improve intent detection: add training examples for this pattern"
                }
                "fact_extraction_incomplete" => {
                    "Enhance fact extraction: parse more content fields"
                }
                "synthesis_hallucination" => {
                    "Add grounding check: verify facts against stored memories"
                }
                "update_not_applied" => {
                    "Fix update pipeline: ensure newer facts override stale ones"
                }
                "contradiction_undetected" => {
                    "Add contradiction detector: compare new facts with existing"
                }
                "procedural_ordering_lost" => {
                    "Preserve step ordering: use indexed sequences in storage"
                }
                "teaching_coverage_gap" => "Expand teaching coverage: add missing topic areas",
                "counterfactual_refusal" => {
                    "Enable hypothetical reasoning: relax factual-only constraints"
                }
                _ => "Review failure manually: no automated fix available",
            };

            analyses.push(FailureAnalysis {
                test_id: test_id.clone(),
                error_category: category.to_string(),
                root_cause,
                suggested_fix: suggested_fix.to_string(),
            });
        }

        // Sort worst-first (unclassified = most concerning)
        analyses.sort_by(|a, b| {
            let a_unknown = a.error_category == "unknown";
            let b_unknown = b.error_category == "unknown";
            b_unknown.cmp(&a_unknown)
        });

        Ok(analyses)
    }
}

/// Proposes code patches based on failure analysis.
pub struct PatchProposer;

impl Default for PatchProposer {
    fn default() -> Self {
        Self::new()
    }
}

impl PatchProposer {
    pub fn new() -> Self {
        Self
    }

    /// Propose patches for the given failure analyses.
    ///
    /// Without an LLM, produces stub proposals with heuristic confidence
    /// scores based on the failure category.
    pub fn propose(&self, analyses: &[FailureAnalysis]) -> Result<Vec<Patch>, EvalError> {
        let mut patches = Vec::new();

        for (i, analysis) in analyses.iter().enumerate() {
            let confidence = match analysis.error_category.as_str() {
                "retrieval_insufficient" => 0.7,
                "temporal_ordering_wrong" => 0.6,
                "fact_extraction_incomplete" => 0.65,
                "synthesis_hallucination" => 0.5,
                "unknown" => 0.3,
                _ => 0.55,
            };

            patches.push(Patch {
                id: format!("patch-{i}-{}", analysis.test_id),
                description: analysis.suggested_fix.clone(),
                diff: format!(
                    "# Proposed fix for {}: {}\n# Category: {}\n# Root cause: {}",
                    analysis.test_id,
                    analysis.suggested_fix,
                    analysis.error_category,
                    analysis.root_cause
                ),
                confidence,
            });
        }

        Ok(patches)
    }
}

/// Multi-reviewer consensus voting on patches.
pub struct ReviewerVoting {
    reviewer_count: u8,
}

impl ReviewerVoting {
    pub fn new(reviewer_count: u8) -> Result<Self, EvalError> {
        if reviewer_count == 0 {
            return Err(EvalError::config("reviewer_count must be > 0"));
        }
        Ok(Self { reviewer_count })
    }

    /// Vote on a patch.
    ///
    /// Without an LLM, uses confidence-based heuristic voting:
    /// high confidence → more approvals, low confidence → more rejections.
    pub fn vote(&self, patch: &Patch) -> Result<VotingResult, EvalError> {
        let mut votes = Vec::with_capacity(self.reviewer_count as usize);

        for i in 0..self.reviewer_count {
            // Simulate 3 reviewer perspectives: quality, regression, simplicity
            let vote = match i % 3 {
                0 => {
                    // Quality reviewer: approve if confidence >= 0.5
                    if patch.confidence >= 0.5 {
                        Vote::Approve
                    } else {
                        Vote::Reject
                    }
                }
                1 => {
                    // Regression reviewer: approve if confidence >= 0.6
                    if patch.confidence >= 0.6 {
                        Vote::Approve
                    } else {
                        Vote::Reject
                    }
                }
                _ => {
                    // Simplicity reviewer: approve if confidence >= 0.4
                    if patch.confidence >= 0.4 {
                        Vote::Approve
                    } else {
                        Vote::Abstain
                    }
                }
            };
            votes.push(vote);
        }

        Ok(VotingResult {
            patch_id: patch.id.clone(),
            votes,
        })
    }

    pub fn reviewer_count(&self) -> u8 {
        self.reviewer_count
    }
}

/// Orchestrates the full self-improvement loop.
pub struct SelfImproveRunner {
    config: SelfImproveConfig,
    analyzer: ErrorAnalyzer,
    proposer: PatchProposer,
    voting: ReviewerVoting,
}

impl SelfImproveRunner {
    pub fn new(config: SelfImproveConfig) -> Result<Self, EvalError> {
        config.validate()?;
        let voting = ReviewerVoting::new(config.reviewer_count)?;
        Ok(Self {
            config,
            analyzer: ErrorAnalyzer::new(),
            proposer: PatchProposer::new(),
            voting,
        })
    }

    /// Run a single improvement iteration.
    pub fn run_iteration(&self, iteration: u32) -> Result<IterationResult, EvalError> {
        // In a real run, this would:
        // 1. Execute eval to get failures
        // 2. Analyze failures
        // 3. Propose patches
        // 4. Vote on patches
        // 5. Apply accepted patches
        // 6. Re-evaluate
        //
        // Without an agent process, we simulate the pipeline structure.
        let simulated_failures = vec![
            (
                "test-recall-1".to_string(),
                "retrieval not found".to_string(),
            ),
            (
                "test-temporal-1".to_string(),
                "wrong order of events".to_string(),
            ),
        ];

        let analyses = self.analyzer.analyze(&simulated_failures)?;
        let patches = self.proposer.propose(&analyses)?;
        let proposed = patches.len();

        let mut applied = 0;
        for patch in &patches {
            let vote_result = self.voting.vote(patch)?;
            if vote_result.approved() && self.config.auto_apply_patches {
                applied += 1;
            }
        }

        Ok(IterationResult {
            iteration,
            score_before: 0.5 + (iteration as f64 * 0.05),
            score_after: 0.55 + (iteration as f64 * 0.05),
            patches_proposed: proposed,
            patches_applied: applied,
            improved: applied > 0,
        })
    }

    /// Run the full improvement loop until target score or max iterations.
    pub fn run(&self) -> Result<Vec<IterationResult>, EvalError> {
        let mut results = Vec::new();

        for i in 0..self.config.max_iterations {
            let result = self.run_iteration(i)?;
            let target_met = result.score_after >= self.config.target_score;
            results.push(result);

            if target_met {
                break;
            }
        }

        Ok(results)
    }

    pub fn config(&self) -> &SelfImproveConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SelfImproveConfig;

    #[test]
    fn voting_result_counts() {
        let vr = VotingResult {
            patch_id: "p1".into(),
            votes: vec![Vote::Approve, Vote::Reject, Vote::Approve, Vote::Abstain],
        };
        assert_eq!(vr.approval_count(), 2);
        assert_eq!(vr.rejection_count(), 1);
    }

    #[test]
    fn voting_result_majority() {
        let approved = VotingResult {
            patch_id: "p1".into(),
            votes: vec![Vote::Approve, Vote::Approve, Vote::Reject],
        };
        assert!(approved.has_majority());

        let rejected = VotingResult {
            patch_id: "p2".into(),
            votes: vec![Vote::Reject, Vote::Reject, Vote::Approve],
        };
        assert!(!rejected.has_majority());
    }

    #[test]
    fn voting_result_all_abstain_no_majority() {
        let vr = VotingResult {
            patch_id: "p1".into(),
            votes: vec![Vote::Abstain, Vote::Abstain],
        };
        assert!(!vr.has_majority());
    }

    #[test]
    fn error_analyzer_sorts_unknown_first() {
        let analyzer = ErrorAnalyzer::new();
        let failures = vec![
            ("t1".into(), "retrieval not found".into()),
            ("t2".into(), "something completely novel".into()),
        ];
        let analyses = analyzer.analyze(&failures).unwrap();
        assert_eq!(analyses[0].error_category, "unknown");
    }

    #[test]
    fn error_analyzer_classifies_retrieval() {
        let analyzer = ErrorAnalyzer::new();
        let failures = vec![("t1".into(), "result not found in store".into())];
        let analyses = analyzer.analyze(&failures).unwrap();
        assert_eq!(analyses[0].error_category, "retrieval_insufficient");
    }

    #[test]
    fn patch_proposer_produces_patches() {
        let analyses = vec![FailureAnalysis {
            test_id: "t1".into(),
            error_category: "retrieval_insufficient".into(),
            root_cause: "missing".into(),
            suggested_fix: "expand search".into(),
        }];
        let proposer = PatchProposer::new();
        let patches = proposer.propose(&analyses).unwrap();
        assert_eq!(patches.len(), 1);
        assert!((patches[0].confidence - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn reviewer_voting_rejects_zero_reviewers() {
        assert!(ReviewerVoting::new(0).is_err());
    }

    #[test]
    fn reviewer_voting_high_confidence_approved() {
        let voting = ReviewerVoting::new(3).unwrap();
        let patch = Patch {
            id: "p1".into(),
            description: "fix".into(),
            diff: "diff".into(),
            confidence: 0.8,
        };
        let result = voting.vote(&patch).unwrap();
        assert!(result.approved());
    }

    #[test]
    fn reviewer_voting_low_confidence_rejected() {
        let voting = ReviewerVoting::new(3).unwrap();
        let patch = Patch {
            id: "p1".into(),
            description: "fix".into(),
            diff: "diff".into(),
            confidence: 0.2,
        };
        let result = voting.vote(&patch).unwrap();
        assert!(!result.approved());
    }

    #[test]
    fn self_improve_runner_rejects_invalid_config() {
        let config = SelfImproveConfig {
            max_iterations: 0,
            ..Default::default()
        };
        assert!(SelfImproveRunner::new(config).is_err());
    }

    #[test]
    fn self_improve_runner_iteration() {
        let config = SelfImproveConfig {
            auto_apply_patches: true,
            ..Default::default()
        };
        let runner = SelfImproveRunner::new(config).unwrap();
        let result = runner.run_iteration(0).unwrap();
        assert_eq!(result.iteration, 0);
        assert!(result.patches_proposed > 0);
    }

    #[test]
    fn self_improve_runner_loop_terminates() {
        let config = SelfImproveConfig {
            max_iterations: 3,
            target_score: 1.0, // unreachable with simulated scores, so runs all iterations
            auto_apply_patches: true,
            ..Default::default()
        };
        let runner = SelfImproveRunner::new(config).unwrap();
        let results = runner.run().unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn self_improve_runner_stops_at_target() {
        let config = SelfImproveConfig {
            max_iterations: 10,
            target_score: 0.0, // immediately met
            auto_apply_patches: true,
            ..Default::default()
        };
        let runner = SelfImproveRunner::new(config).unwrap();
        let results = runner.run().unwrap();
        // Should stop after first iteration since score_after >= 0.0
        assert_eq!(results.len(), 1);
    }
}
