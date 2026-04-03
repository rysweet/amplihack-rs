//! Tests for the grading infrastructure.

use amplihack_agent_eval::EvalError;
use amplihack_agent_eval::grader::{Grader, SimpleGrader, grade_with_votes};
use amplihack_agent_eval::levels::TestLevel;
use amplihack_agent_eval::models::GradeResult;

// ── SimpleGrader construction ────────────────────────────────

#[test]
fn simple_grader_valid_construction() {
    let g = SimpleGrader::new(3);
    assert!(g.is_ok());
    assert_eq!(g.unwrap().votes, 3);
}

#[test]
fn simple_grader_rejects_zero_votes() {
    assert!(SimpleGrader::new(0).is_err());
}

#[test]
fn simple_grader_single_vote() {
    let g = SimpleGrader::new(1).unwrap();
    assert_eq!(g.votes, 1);
}

// ── Grading behavior ─────────────────────────────────────────

#[test]
fn grade_exact_match_returns_score_one() {
    let grader = SimpleGrader::new(1).unwrap();
    let result = grader
        .grade("What is 2+2?", "4", "4", TestLevel::L1Recall)
        .unwrap();
    assert!((result.score - 1.0).abs() < f64::EPSILON);
    assert!(result.reasoning.contains("Exact match"));
}

#[test]
fn grade_complete_miss_returns_zero() {
    let grader = SimpleGrader::new(1).unwrap();
    let result = grader
        .grade("What is 2+2?", "4", "purple elephant", TestLevel::L1Recall)
        .unwrap();
    // No word overlap between {"4"} and {"purple", "elephant"} → 0.0 * 0.8 = 0.0
    assert!((result.score).abs() < f64::EPSILON);
}

#[test]
fn grade_partial_word_overlap_returns_fractional_score() {
    let grader = SimpleGrader::new(1).unwrap();
    let result = grader
        .grade(
            "Name the planets",
            "Mercury, Venus, Earth, Mars",
            "Mercury, Venus, Earth",
            TestLevel::L2MultiSourceSynthesis,
        )
        .unwrap();
    // Word overlap scoring with partial match; score should be between 0 and 1 exclusive
    assert!(result.score > 0.0);
    assert!(result.score < 1.0);
}

#[test]
fn grade_empty_actual_returns_zero() {
    let grader = SimpleGrader::new(1).unwrap();
    let result = grader
        .grade("What is X?", "Something", "", TestLevel::L1Recall)
        .unwrap();
    assert!((result.score).abs() < f64::EPSILON);
    assert!(result.reasoning.contains("Empty answer"));
}

#[test]
fn grade_temporal_level_applies_bonus() {
    let grader = SimpleGrader::new(1).unwrap();
    // "before" appears in both expected and actual → positive temporal bonus
    let with_temporal = grader
        .grade(
            "What happened first?",
            "A happened before B",
            "A happened before B then C",
            TestLevel::L3TemporalReasoning,
        )
        .unwrap();
    // Same content without temporal level for comparison
    let without_temporal = grader
        .grade(
            "What happened first?",
            "A happened before B",
            "A happened before B then C",
            TestLevel::L1Recall,
        )
        .unwrap();
    // L3 temporal bonus (+0.1) should make score higher than the same grading at L1
    assert!(with_temporal.score > without_temporal.score);
}

#[test]
fn grade_contradiction_level_applies_bonus() {
    let grader = SimpleGrader::new(1).unwrap();
    let result = grader
        .grade(
            "Resolve the contradiction",
            "Source A is outdated",
            "Source A is outdated, so Source B is correct",
            TestLevel::L5ContradictionHandling,
        )
        .unwrap();
    // actual contains expected → 0.9 base, plus L5 bonus for "outdated" keyword → 1.0
    assert!((result.score - 1.0).abs() < f64::EPSILON);
}

#[test]
fn grade_containment_returns_high_score() {
    let grader = SimpleGrader::new(1).unwrap();
    let result = grader
        .grade(
            "What is Rust?",
            "systems language",
            "Rust is a systems language for safe concurrency",
            TestLevel::L1Recall,
        )
        .unwrap();
    // actual contains expected → base score 0.9
    assert!((result.score - 0.9).abs() < f64::EPSILON);
}

// ── Multi-vote grading ───────────────────────────────────────

#[test]
fn grade_with_votes_returns_median_and_vote_scores() {
    let grader = SimpleGrader::new(1).unwrap();
    let result = grade_with_votes(
        &grader,
        "What is X?",
        "Answer",
        "Answer",
        TestLevel::L1Recall,
        3,
    )
    .unwrap();
    // Exact match each time → all votes are 1.0, median is 1.0
    assert!((result.score - 1.0).abs() < f64::EPSILON);
    let votes = result.vote_scores.unwrap();
    assert_eq!(votes.len(), 3);
    for v in &votes {
        assert!((*v - 1.0).abs() < f64::EPSILON);
    }
}

#[test]
fn grade_with_votes_single_vote() {
    let grader = SimpleGrader::new(1).unwrap();
    let result = grade_with_votes(&grader, "q", "a", "", TestLevel::L1Recall, 1).unwrap();
    // Empty actual → score 0.0
    assert!((result.score).abs() < f64::EPSILON);
    assert_eq!(result.vote_scores.unwrap().len(), 1);
}

// ── Trait object usage ───────────────────────────────────────

#[test]
fn grader_is_object_safe() {
    let grader: Box<dyn Grader> = Box::new(SimpleGrader::new(1).unwrap());
    let result = grader.grade("q", "e", "e", TestLevel::L1Recall).unwrap();
    assert!((result.score - 1.0).abs() < f64::EPSILON);
}

// ── Test doubles ─────────────────────────────────────────────

struct FixedGrader {
    score: f64,
}

impl Grader for FixedGrader {
    fn grade(
        &self,
        _question: &str,
        _expected: &str,
        _actual: &str,
        _level: TestLevel,
    ) -> Result<GradeResult, EvalError> {
        GradeResult::new(self.score, "Fixed grade")
    }
}

#[test]
fn fixed_grader_always_returns_score() {
    let grader = FixedGrader { score: 0.42 };
    let result = grader.grade("q", "e", "a", TestLevel::L1Recall).unwrap();
    assert!((result.score - 0.42).abs() < f64::EPSILON);
}

#[test]
fn fixed_grader_perfect_score() {
    let grader = FixedGrader { score: 1.0 };
    let result = grader
        .grade("q", "e", "a", TestLevel::L12FarTransfer)
        .unwrap();
    assert!(result.passed(0.5));
}

#[test]
fn fixed_grader_zero_score() {
    let grader = FixedGrader { score: 0.0 };
    let result = grader.grade("q", "e", "a", TestLevel::L1Recall).unwrap();
    assert!(!result.passed(0.1));
}

#[test]
fn fixed_grader_as_trait_object() {
    let grader: Box<dyn Grader> = Box::new(FixedGrader { score: 0.75 });
    let result = grader
        .grade("q", "e", "a", TestLevel::L4ProceduralLearning)
        .unwrap();
    assert!((result.score - 0.75).abs() < f64::EPSILON);
}
