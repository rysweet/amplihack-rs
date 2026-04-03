//! Tests for all model types in the eval framework.

use amplihack_agent_eval::levels::TestLevel;
use amplihack_agent_eval::models::*;
use std::path::PathBuf;
#[test]
fn grade_result_valid_construction() {
    let gr = GradeResult::new(0.85, "Good answer").unwrap();
    assert!((gr.score - 0.85).abs() < f64::EPSILON);
    assert_eq!(gr.reasoning, "Good answer");
    assert!(gr.vote_scores.is_none());
}

#[test]
fn grade_result_score_zero() {
    let gr = GradeResult::new(0.0, "Complete miss").unwrap();
    assert!((gr.score).abs() < f64::EPSILON);
}

#[test]
fn grade_result_score_one() {
    let gr = GradeResult::new(1.0, "Perfect match").unwrap();
    assert!((gr.score - 1.0).abs() < f64::EPSILON);
}

#[test]
fn grade_result_rejects_score_above_one() {
    assert!(GradeResult::new(1.1, "Too high").is_err());
}

#[test]
fn grade_result_rejects_negative_score() {
    assert!(GradeResult::new(-0.1, "Negative").is_err());
}

#[test]
fn grade_result_rejects_empty_reasoning() {
    assert!(GradeResult::new(0.5, "").is_err());
}

#[test]
fn grade_result_with_votes() {
    let gr = GradeResult::new(0.8, "Averaged")
        .unwrap()
        .with_votes(vec![0.7, 0.8, 0.9]);
    let votes = gr.vote_scores.unwrap();
    assert_eq!(votes.len(), 3);
}

#[test]
fn grade_result_passed_at_threshold() {
    let gr = GradeResult::new(0.7, "At threshold").unwrap();
    assert!(gr.passed(0.7));
}

#[test]
fn grade_result_failed_below_threshold() {
    let gr = GradeResult::new(0.69, "Below threshold").unwrap();
    assert!(!gr.passed(0.7));
}

#[test]
fn grade_result_serde_roundtrip() {
    let gr = GradeResult::new(0.75, "Decent answer")
        .unwrap()
        .with_votes(vec![0.7, 0.8]);
    let json = serde_json::to_string(&gr).unwrap();
    let deser: GradeResult = serde_json::from_str(&json).unwrap();
    assert!((deser.score - 0.75).abs() < f64::EPSILON);
    assert_eq!(deser.reasoning, "Decent answer");
    assert_eq!(deser.vote_scores.unwrap().len(), 2);
}

#[test]
fn grade_result_json_format_snake_case() {
    let gr = GradeResult::new(0.5, "test").unwrap().with_votes(vec![0.5]);
    let json = serde_json::to_string(&gr).unwrap();
    assert!(json.contains("\"vote_scores\""));
    assert!(!json.contains("\"voteScores\""));
}

#[test]
fn grade_result_json_omits_none_votes() {
    let gr = GradeResult::new(0.5, "test").unwrap();
    let json = serde_json::to_string(&gr).unwrap();
    assert!(!json.contains("vote_scores"));
}
#[test]
fn test_question_valid_construction() {
    let q = TestQuestion::new("q1", "What is X?", TestLevel::L1Recall).unwrap();
    assert_eq!(q.id, "q1");
    assert_eq!(q.question, "What is X?");
    assert!(q.context.is_none());
}

#[test]
fn test_question_with_context() {
    let q = TestQuestion::new("q2", "What?", TestLevel::L1Recall)
        .unwrap()
        .with_context("Some background");
    assert_eq!(q.context.unwrap(), "Some background");
}

#[test]
fn test_question_rejects_empty_id() {
    assert!(TestQuestion::new("", "What?", TestLevel::L1Recall).is_err());
}

#[test]
fn test_question_rejects_empty_question() {
    assert!(TestQuestion::new("q1", "", TestLevel::L1Recall).is_err());
}

#[test]
fn test_question_serde_roundtrip() {
    let q = TestQuestion::new("q1", "What?", TestLevel::L3TemporalReasoning)
        .unwrap()
        .with_context("ctx");
    let json = serde_json::to_string(&q).unwrap();
    let deser: TestQuestion = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.id, "q1");
    assert_eq!(deser.level, TestLevel::L3TemporalReasoning);
    assert_eq!(deser.context.unwrap(), "ctx");
}
#[test]
fn test_case_valid_construction() {
    let q = TestQuestion::new("q1", "What?", TestLevel::L1Recall).unwrap();
    let tc = TestCase::new(q, "The answer").unwrap();
    assert_eq!(tc.expected_answer, "The answer");
    assert!(tc.tags.is_empty());
}

#[test]
fn test_case_rejects_empty_answer() {
    let q = TestQuestion::new("q1", "What?", TestLevel::L1Recall).unwrap();
    assert!(TestCase::new(q, "").is_err());
}

#[test]
fn test_case_with_tags() {
    let q = TestQuestion::new("q1", "What?", TestLevel::L1Recall).unwrap();
    let tc = TestCase::new(q, "Answer")
        .unwrap()
        .with_tags(vec!["memory".into(), "recall".into()]);
    assert_eq!(tc.tags.len(), 2);
}

#[test]
fn test_case_serde_roundtrip() {
    let q = TestQuestion::new("q1", "What?", TestLevel::L2MultiSourceSynthesis).unwrap();
    let tc = TestCase::new(q, "Combined answer")
        .unwrap()
        .with_tags(vec!["synthesis".into()]);
    let json = serde_json::to_string(&tc).unwrap();
    let deser: TestCase = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.expected_answer, "Combined answer");
    assert_eq!(deser.tags, vec!["synthesis"]);
}
#[test]
fn level_result_passed() {
    let lr = LevelResult::passed(TestLevel::L1Recall, vec![0.9, 0.95]);
    assert!(lr.success);
    assert_eq!(lr.level_id, 1);
    assert_eq!(lr.level_name, "Recall");
    assert!(lr.error_message.is_none());
}

#[test]
fn level_result_failed() {
    let lr = LevelResult::failed(TestLevel::L5ContradictionHandling, "timeout");
    assert!(!lr.success);
    assert_eq!(lr.level_id, 5);
    assert_eq!(lr.error_message.unwrap(), "timeout");
}

#[test]
fn level_result_average_score() {
    let lr = LevelResult::passed(TestLevel::L1Recall, vec![0.8, 0.9, 1.0]);
    assert!((lr.average_score() - 0.9).abs() < f64::EPSILON);
}

#[test]
fn level_result_average_score_empty() {
    let lr = LevelResult::failed(TestLevel::L1Recall, "err");
    assert!((lr.average_score()).abs() < f64::EPSILON);
}

#[test]
fn level_result_serde_roundtrip() {
    let lr = LevelResult::passed(TestLevel::L3TemporalReasoning, vec![0.8]);
    let json = serde_json::to_string(&lr).unwrap();
    let deser: LevelResult = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.level_id, 3);
    assert!(deser.success);
}
#[test]
fn progressive_config_defaults() {
    let cfg = ProgressiveConfig::new("test-agent", PathBuf::from("./out")).unwrap();
    assert_eq!(cfg.agent_name, "test-agent");
    assert_eq!(cfg.levels_to_run.len(), 12);
    assert_eq!(cfg.grader_votes, 3);
    assert_eq!(cfg.sdk, "default");
}

#[test]
fn progressive_config_rejects_empty_name() {
    assert!(ProgressiveConfig::new("", PathBuf::from("./out")).is_err());
}

#[test]
fn progressive_config_with_levels() {
    let cfg = ProgressiveConfig::new("agent", PathBuf::from("./out"))
        .unwrap()
        .with_levels(vec![TestLevel::L1Recall, TestLevel::L2MultiSourceSynthesis]);
    assert_eq!(cfg.levels_to_run.len(), 2);
}

#[test]
fn progressive_config_serde_roundtrip() {
    let cfg = ProgressiveConfig::new("agent", PathBuf::from("./out"))
        .unwrap()
        .with_sdk("openai")
        .with_grader_votes(5);
    let json = serde_json::to_string(&cfg).unwrap();
    let deser: ProgressiveConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.sdk, "openai");
    assert_eq!(deser.grader_votes, 5);
}
#[test]
fn progressive_result_add_passed() {
    let cfg = ProgressiveConfig::new("agent", PathBuf::from("./out")).unwrap();
    let mut pr = ProgressiveResult::new(cfg);
    pr.add_result(LevelResult::passed(TestLevel::L1Recall, vec![0.95]));
    assert_eq!(pr.passed_levels, vec![1]);
    assert!(pr.failed_levels.is_empty());
    assert!((pr.total_score - 0.95).abs() < f64::EPSILON);
}

#[test]
fn progressive_result_add_failed() {
    let cfg = ProgressiveConfig::new("agent", PathBuf::from("./out")).unwrap();
    let mut pr = ProgressiveResult::new(cfg);
    pr.add_result(LevelResult::failed(TestLevel::L2MultiSourceSynthesis, "err"));
    assert!(pr.passed_levels.is_empty());
    assert_eq!(pr.failed_levels, vec![2]);
}

#[test]
fn progressive_result_total_score_averaged() {
    let cfg = ProgressiveConfig::new("agent", PathBuf::from("./out")).unwrap();
    let mut pr = ProgressiveResult::new(cfg);
    pr.add_result(LevelResult::passed(TestLevel::L1Recall, vec![1.0]));
    pr.add_result(LevelResult::passed(TestLevel::L2MultiSourceSynthesis, vec![0.5]));
    assert!((pr.total_score - 0.75).abs() < f64::EPSILON);
}

#[test]
fn progressive_result_finish_sets_timestamp() {
    let cfg = ProgressiveConfig::new("agent", PathBuf::from("./out")).unwrap();
    let mut pr = ProgressiveResult::new(cfg);
    assert!(pr.finished_at.is_none());
    pr.finish();
    assert!(pr.finished_at.is_some());
}

#[test]
fn progressive_result_serde_roundtrip() {
    let cfg = ProgressiveConfig::new("agent", PathBuf::from("./out")).unwrap();
    let mut pr = ProgressiveResult::new(cfg);
    pr.add_result(LevelResult::passed(TestLevel::L1Recall, vec![0.9]));
    pr.finish();
    let json = serde_json::to_string(&pr).unwrap();
    let deser: ProgressiveResult = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.level_results.len(), 1);
    assert!(deser.finished_at.is_some());
}
#[test]
fn harness_config_defaults() {
    let hc = HarnessConfig::default();
    assert_eq!(hc.timeout_seconds, 300);
    assert_eq!(hc.retries, 3);
}

#[test]
fn harness_config_valid_construction() {
    let hc = HarnessConfig::new("suite1", "agent.toml").unwrap();
    assert_eq!(hc.test_suite, "suite1");
    assert_eq!(hc.agent_config, "agent.toml");
}

#[test]
fn harness_config_rejects_empty_suite() {
    assert!(HarnessConfig::new("", "agent.toml").is_err());
}

#[test]
fn harness_config_rejects_empty_agent_config() {
    assert!(HarnessConfig::new("suite1", "").is_err());
}

#[test]
fn harness_config_builder() {
    let hc = HarnessConfig::new("suite1", "agent.toml")
        .unwrap()
        .with_timeout(600)
        .with_retries(5);
    assert_eq!(hc.timeout_seconds, 600);
    assert_eq!(hc.retries, 5);
}

#[test]
fn harness_config_serde_roundtrip() {
    let hc = HarnessConfig::new("suite1", "agent.toml")
        .unwrap()
        .with_timeout(120);
    let json = serde_json::to_string(&hc).unwrap();
    let deser: HarnessConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.timeout_seconds, 120);
    assert_eq!(deser.test_suite, "suite1");
}
#[test]
fn self_improve_config_defaults() {
    let cfg = SelfImproveConfig::default();
    assert_eq!(cfg.max_iterations, 5);
    assert!((cfg.target_score - 0.8).abs() < f64::EPSILON);
    assert_eq!(cfg.reviewer_count, 3);
    assert!(!cfg.auto_apply_patches);
}

#[test]
fn self_improve_config_validates_ok() {
    assert!(SelfImproveConfig::default().validate().is_ok());
}

#[test]
fn self_improve_config_rejects_zero_iterations() {
    let mut cfg = SelfImproveConfig::default();
    cfg.max_iterations = 0;
    assert!(cfg.validate().is_err());
}

#[test]
fn self_improve_config_rejects_invalid_target() {
    let mut cfg = SelfImproveConfig::default();
    cfg.target_score = 1.5;
    assert!(cfg.validate().is_err());
}

#[test]
fn self_improve_config_rejects_zero_reviewers() {
    let mut cfg = SelfImproveConfig::default();
    cfg.reviewer_count = 0;
    assert!(cfg.validate().is_err());
}

#[test]
fn self_improve_config_serde_roundtrip() {
    let cfg = SelfImproveConfig::default();
    let json = serde_json::to_string(&cfg).unwrap();
    let deser: SelfImproveConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.max_iterations, cfg.max_iterations);
}
#[test]
fn grade_result_python_json_compat() {
    // Simulates the JSON format the Python grader produces
    let python_json = r#"{
        "score": 0.85,
        "reasoning": "Good answer with relevant details",
        "vote_scores": [0.8, 0.9, 0.85]
    }"#;
    let gr: GradeResult = serde_json::from_str(python_json).unwrap();
    assert!((gr.score - 0.85).abs() < f64::EPSILON);
    assert_eq!(gr.vote_scores.unwrap().len(), 3);
}

#[test]
fn harness_config_python_json_compat() {
    let python_json = r#"{
        "test_suite": "basic",
        "agent_config": "gpt4.yaml",
        "timeout_seconds": 600,
        "retries": 2
    }"#;
    let hc: HarnessConfig = serde_json::from_str(python_json).unwrap();
    assert_eq!(hc.test_suite, "basic");
    assert_eq!(hc.timeout_seconds, 600);
}
