//! Tests for the progressive test suite.

use amplihack_agent_eval::EvalError;
use amplihack_agent_eval::grader::Grader;
use amplihack_agent_eval::levels::TestLevel;
use amplihack_agent_eval::models::*;
use amplihack_agent_eval::progressive::ProgressiveSuite;
use std::path::PathBuf;

// ── Test double ──────────────────────────────────────────────

struct StubGrader;

impl Grader for StubGrader {
    fn grade(
        &self,
        _q: &str,
        _e: &str,
        _a: &str,
        _level: TestLevel,
    ) -> Result<GradeResult, EvalError> {
        GradeResult::new(0.8, "Stub grade")
    }
}

fn make_config() -> ProgressiveConfig {
    ProgressiveConfig::new("test-agent", PathBuf::from("./out")).unwrap()
}

fn make_test_case(level: TestLevel) -> TestCase {
    let q = TestQuestion::new(
        format!("q-{}", level.id()),
        format!("Question for {}", level),
        level,
    )
    .unwrap();
    TestCase::new(q, "Expected answer").unwrap()
}

fn make_suite(levels: &[TestLevel]) -> ProgressiveSuite {
    let config = make_config().with_levels(levels.to_vec());
    let cases: Vec<TestCase> = levels.iter().map(|l| make_test_case(*l)).collect();
    ProgressiveSuite::new(config, cases, Box::new(StubGrader))
}

// ── Construction ─────────────────────────────────────────────

#[test]
fn suite_construction() {
    let suite = make_suite(&[TestLevel::L1Recall]);
    assert_eq!(suite.config().agent_name, "test-agent");
}

#[test]
fn suite_cases_for_level() {
    let suite = make_suite(&[TestLevel::L1Recall, TestLevel::L2MultiSourceSynthesis]);
    let l1_cases = suite.cases_for_level(TestLevel::L1Recall);
    assert_eq!(l1_cases.len(), 1);
    let l3_cases = suite.cases_for_level(TestLevel::L3TemporalReasoning);
    assert_eq!(l3_cases.len(), 0);
}

#[test]
fn suite_config_levels() {
    let suite = make_suite(&[TestLevel::L1Recall, TestLevel::L3TemporalReasoning]);
    assert_eq!(suite.config().levels_to_run.len(), 2);
}

// ── run_level ────────────────────────────────────────────────

#[test]
fn run_level_l1_fails_below_threshold() {
    // StubGrader returns 0.8, L1 threshold is 0.9 → should fail
    let suite = make_suite(&[TestLevel::L1Recall]);
    let result = suite.run_level(TestLevel::L1Recall).unwrap();
    assert!(!result.success);
    assert_eq!(result.level_id, 1);
    assert!(result.error_message.is_some());
}

#[test]
fn run_level_l12_passes_with_stub_grader() {
    // StubGrader returns 0.8, L12 threshold is 0.5 → should pass
    let suite = make_suite(&[TestLevel::L12FarTransfer]);
    let result = suite.run_level(TestLevel::L12FarTransfer).unwrap();
    assert!(result.success);
    assert_eq!(result.level_id, 12);
    assert!(result.error_message.is_none());
    assert!(!result.scores.is_empty());
}

#[test]
fn run_level_missing_cases_returns_error() {
    let suite = make_suite(&[TestLevel::L1Recall]);
    // L3 has no test cases in this suite
    let result = suite.run_level(TestLevel::L3TemporalReasoning);
    assert!(result.is_err());
}

// ── run_all ──────────────────────────────────────────────────

#[test]
fn run_all_returns_results_for_all_levels() {
    let suite = make_suite(TestLevel::all());
    let result = suite.run_all().unwrap();
    // StubGrader returns 0.8 for all → levels with threshold <= 0.8 pass
    // L1 (0.9) and L2 (0.85) should fail; L3-L12 (thresholds 0.5-0.8) should pass
    assert_eq!(result.level_results.len(), 12);
    assert_eq!(result.failed_levels.len(), 2); // L1, L2
    assert_eq!(result.passed_levels.len(), 10); // L3-L12
    assert!(result.finished_at.is_some());
}

#[test]
fn run_all_with_single_level() {
    let suite = make_suite(&[TestLevel::L5ContradictionHandling]);
    let result = suite.run_all().unwrap();
    // StubGrader 0.8 >= L5 threshold 0.7 → pass
    assert_eq!(result.level_results.len(), 1);
    assert_eq!(result.passed_levels.len(), 1);
    assert!(result.failed_levels.is_empty());
}

// ── compute_summary ──────────────────────────────────────────

#[test]
fn summary_empty_results() {
    let summary = ProgressiveSuite::compute_summary(&[]);
    assert_eq!(summary.total_levels, 0);
    assert_eq!(summary.passed_levels, 0);
    assert!((summary.average_score).abs() < f64::EPSILON);
}

#[test]
fn summary_all_passed() {
    let results = vec![
        LevelResult::passed(TestLevel::L1Recall, vec![0.9]),
        LevelResult::passed(TestLevel::L2MultiSourceSynthesis, vec![0.85]),
    ];
    let summary = ProgressiveSuite::compute_summary(&results);
    assert_eq!(summary.total_levels, 2);
    assert_eq!(summary.passed_levels, 2);
    assert_eq!(summary.failed_levels, 0);
    assert!((summary.average_score - 0.875).abs() < f64::EPSILON);
}

#[test]
fn summary_mixed_results() {
    let results = vec![
        LevelResult::passed(TestLevel::L1Recall, vec![1.0]),
        LevelResult::failed(TestLevel::L2MultiSourceSynthesis, "err"),
    ];
    let summary = ProgressiveSuite::compute_summary(&results);
    assert_eq!(summary.passed_levels, 1);
    assert_eq!(summary.failed_levels, 1);
    // (1.0 + 0.0) / 2 = 0.5
    assert!((summary.average_score - 0.5).abs() < f64::EPSILON);
}

#[test]
fn summary_all_failed() {
    let results = vec![
        LevelResult::failed(TestLevel::L1Recall, "e1"),
        LevelResult::failed(TestLevel::L2MultiSourceSynthesis, "e2"),
    ];
    let summary = ProgressiveSuite::compute_summary(&results);
    assert_eq!(summary.passed_levels, 0);
    assert_eq!(summary.failed_levels, 2);
    assert!((summary.average_score).abs() < f64::EPSILON);
}

#[test]
fn summary_single_result() {
    let results = vec![LevelResult::passed(
        TestLevel::L5ContradictionHandling,
        vec![0.72],
    )];
    let summary = ProgressiveSuite::compute_summary(&results);
    assert_eq!(summary.total_levels, 1);
    assert!((summary.average_score - 0.72).abs() < f64::EPSILON);
}

// ── Config validation ────────────────────────────────────────

#[test]
fn config_with_empty_levels_is_valid() {
    let config = make_config().with_levels(vec![]);
    assert!(config.levels_to_run.is_empty());
}

#[test]
fn config_grader_votes_override() {
    let config = make_config().with_grader_votes(7);
    assert_eq!(config.grader_votes, 7);
}

// ── ProgressiveResult aggregation ────────────────────────────

#[test]
fn progressive_result_score_recomputes_on_each_add() {
    let mut pr = ProgressiveResult::new(make_config());
    pr.add_result(LevelResult::passed(TestLevel::L1Recall, vec![1.0]));
    assert!((pr.total_score - 1.0).abs() < f64::EPSILON);
    pr.add_result(LevelResult::passed(
        TestLevel::L2MultiSourceSynthesis,
        vec![0.0],
    ));
    assert!((pr.total_score - 0.5).abs() < f64::EPSILON);
}

#[test]
fn progressive_result_tracks_passed_and_failed_ids() {
    let mut pr = ProgressiveResult::new(make_config());
    pr.add_result(LevelResult::passed(TestLevel::L1Recall, vec![0.95]));
    pr.add_result(LevelResult::failed(
        TestLevel::L3TemporalReasoning,
        "timeout",
    ));
    pr.add_result(LevelResult::passed(
        TestLevel::L4ProceduralLearning,
        vec![0.8],
    ));
    assert_eq!(pr.passed_levels, vec![1, 4]);
    assert_eq!(pr.failed_levels, vec![3]);
}
