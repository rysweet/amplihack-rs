//! Compatibility layer between eval versions.
//!
//! Ports Python `amplihack/evaluation/compat.py`.
//!
//! Provides re-exports and type aliases so that code written against an older
//! or external `amplihack-eval` crate can compile against this module without
//! changes. All canonical types live in sibling modules; this module simply
//! centralises the public surface for downstream consumers that `use compat::*`.

// Re-export core types under their original external-crate names.

pub use crate::agent_adapter::{AgentAdapter, AgentResponse};
pub use crate::grader::Grader;
pub use crate::long_horizon::{
    DimensionScore, GradingRubric, LongHorizonConfig, LongHorizonQuestion, LongHorizonReport,
};
pub use crate::models::GradeResult;
pub use crate::self_improve::{Patch, PatchProposer, ReviewerVoting, Vote, VotingResult};

// ---------------------------------------------------------------------------
// Type aliases preserving names from the standalone `amplihack_eval` crate
// ---------------------------------------------------------------------------

/// Alias for [`AgentResponse`], matching the external crate's `ToolCall` name.
pub type ToolCall = AgentResponse;

/// Alias for [`LongHorizonQuestion`], matching the external crate's `Question`.
pub type Question = LongHorizonQuestion;

/// Alias for [`GradingRubric`], matching the external crate's `GroundTruth`.
pub type GroundTruth = GradingRubric;

/// Alias for [`Vote`], matching the external crate's `ReviewVote`.
pub type ReviewVote = Vote;

/// Alias for [`VotingResult`], matching the external crate's `ReviewResult`.
pub type ReviewResult = VotingResult;

/// Alias for [`Patch`], matching the external crate's `PatchProposal`.
pub type PatchProposal = Patch;

/// Check that the compat layer is operational.
///
/// Returns `true` unconditionally — the Rust crate always provides every type.
/// This mirrors the Python fallback that yields `False` when the external
/// package is not installed.
pub fn is_available() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compat_is_available() {
        assert!(is_available());
    }

    #[test]
    fn type_aliases_resolve() {
        // Ensure type aliases compile and are usable.
        fn _accept_tool_call(_tc: ToolCall) {}
        fn _accept_question(_q: Question) {}
        fn _accept_ground_truth(_gt: GroundTruth) {}
        fn _accept_review_vote(_rv: ReviewVote) {}
        fn _accept_review_result(_rr: ReviewResult) {}
        fn _accept_patch_proposal(_pp: PatchProposal) {}
    }

    #[test]
    fn grade_result_via_compat() {
        let gr = GradeResult::new(0.9, "good").unwrap();
        assert!(gr.passed(0.8));
    }

    #[test]
    fn dimension_score_via_compat() {
        let ds = DimensionScore::new("factual_accuracy", 0.85, "correct");
        assert_eq!(ds.dimension, "factual_accuracy");
        assert!((ds.score - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn agent_response_as_tool_call() {
        let tc: ToolCall = AgentResponse::new("answer");
        assert_eq!(tc.answer, "answer");
    }

    #[test]
    fn grading_rubric_as_ground_truth() {
        let gt: GroundTruth = GradingRubric {
            required_keywords: vec!["rust".into()],
            ..Default::default()
        };
        assert_eq!(gt.required_keywords.len(), 1);
    }
}
