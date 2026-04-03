//! Tests for the grading infrastructure.

use amplihack_agent_eval::grader::{grade_with_votes, Grader, SimpleGrader};
use amplihack_agent_eval::levels::TestLevel;
use amplihack_agent_eval::models::GradeResult;
use amplihack_agent_eval::EvalError;

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

// ── Grading (should_panic since todo!()) ─────────────────────

#[test]
#[should_panic(expected = "not yet implemented")]
fn grade_perfect_match_panics_todo() {
    let grader = SimpleGrader::new(1).unwrap();
    let _ = grader.grade(
        "What is 2+2?",
        "4",
        "4",
        TestLevel::L1Recall,
    );
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn grade_complete_miss_panics_todo() {
    let grader = SimpleGrader::new(1).unwrap();
    let _ = grader.grade(
        "What is 2+2?",
        "4",
        "purple elephant",
        TestLevel::L1Recall,
    );
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn grade_partial_match_panics_todo() {
    let grader = SimpleGrader::new(1).unwrap();
    let _ = grader.grade(
        "Name the planets",
        "Mercury, Venus, Earth, Mars",
        "Mercury, Venus, Earth",
        TestLevel::L2MultiSourceSynthesis,
    );
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn grade_empty_actual_panics_todo() {
    let grader = SimpleGrader::new(1).unwrap();
    let _ = grader.grade(
        "What is X?",
        "Something",
        "",
        TestLevel::L1Recall,
    );
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn grade_temporal_level_panics_todo() {
    let grader = SimpleGrader::new(1).unwrap();
    let _ = grader.grade(
        "What happened first?",
        "Event A before Event B",
        "Event A happened first",
        TestLevel::L3TemporalReasoning,
    );
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn grade_contradiction_level_panics_todo() {
    let grader = SimpleGrader::new(1).unwrap();
    let _ = grader.grade(
        "Resolve the contradiction",
        "Source A is outdated",
        "Source A is outdated, so Source B is correct",
        TestLevel::L5ContradictionHandling,
    );
}

// ── Multi-vote grading ───────────────────────────────────────

#[test]
#[should_panic(expected = "not yet implemented")]
fn grade_with_votes_panics_todo() {
    let grader = SimpleGrader::new(1).unwrap();
    let _ = grade_with_votes(
        &grader,
        "What is X?",
        "Answer",
        "Answer",
        TestLevel::L1Recall,
        3,
    );
}

// ── Trait object usage ───────────────────────────────────────

#[test]
fn grader_is_object_safe() {
    let grader: Box<dyn Grader> = Box::new(SimpleGrader::new(1).unwrap());
    // Ensure we can hold a trait object (the call will panic due to todo!())
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        grader.grade("q", "e", "a", TestLevel::L1Recall)
    }));
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
    let result = grader
        .grade("q", "e", "a", TestLevel::L1Recall)
        .unwrap();
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
    let result = grader
        .grade("q", "e", "a", TestLevel::L1Recall)
        .unwrap();
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
