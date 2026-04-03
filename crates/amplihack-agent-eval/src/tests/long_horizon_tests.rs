//! Tests for long_horizon module.

use super::*;
use std::collections::HashMap;

#[test]
fn dimension_score_clamped() {
    let ds = DimensionScore::new("factual_accuracy", 1.5, "over max");
    assert!((ds.score - 1.0).abs() < f64::EPSILON);

    let ds = DimensionScore::new("factual_accuracy", -0.5, "under min");
    assert!((ds.score - 0.0).abs() < f64::EPSILON);
}

#[test]
fn eval_result_compute_overall() {
    let mut result = EvalResult {
        question_id: "Q1".into(),
        question_text: "test".into(),
        category: "cat".into(),
        expected_answer: "expected".into(),
        actual_answer: "actual".into(),
        dimensions: vec![
            DimensionScore::new("factual_accuracy", 0.8, "good"),
            DimensionScore::new("specificity", 0.6, "ok"),
        ],
        overall_score: 0.0,
        grading_time_s: 0.0,
    };
    result.compute_overall();
    assert!((result.overall_score - 0.7).abs() < 0.001);
}

#[test]
fn eval_result_compute_overall_empty() {
    let mut result = EvalResult {
        question_id: "Q2".into(),
        question_text: "test".into(),
        category: "cat".into(),
        expected_answer: "exp".into(),
        actual_answer: "act".into(),
        dimensions: vec![],
        overall_score: 0.0,
        grading_time_s: 0.0,
    };
    result.compute_overall();
    assert!((result.overall_score).abs() < f64::EPSILON);
}

#[test]
fn long_horizon_report_compute_breakdowns() {
    let mut report = LongHorizonReport {
        num_turns: 100,
        num_questions: 2,
        total_facts_delivered: 50,
        learning_time_s: 1.0,
        questioning_time_s: 0.5,
        grading_time_s: 0.2,
        overall_score: 0.0,
        category_breakdown: vec![],
        results: vec![
            EvalResult {
                question_id: "Q1".into(),
                question_text: "q1".into(),
                category: "factual".into(),
                expected_answer: "a".into(),
                actual_answer: "a".into(),
                dimensions: vec![DimensionScore::new("factual_accuracy", 0.9, "good")],
                overall_score: 0.9,
                grading_time_s: 0.0,
            },
            EvalResult {
                question_id: "Q2".into(),
                question_text: "q2".into(),
                category: "factual".into(),
                expected_answer: "b".into(),
                actual_answer: "c".into(),
                dimensions: vec![DimensionScore::new("factual_accuracy", 0.5, "ok")],
                overall_score: 0.5,
                grading_time_s: 0.0,
            },
        ],
        memory_stats: HashMap::new(),
    };

    report.compute_breakdowns();
    assert!((report.overall_score - 0.7).abs() < 0.001);
    assert_eq!(report.category_breakdown.len(), 1);
    assert_eq!(report.category_breakdown[0].num_questions, 2);
}

#[test]
fn grading_rubric_default() {
    let rubric = GradingRubric::default();
    assert!(rubric.required_keywords.is_empty());
    assert!(rubric.acceptable_paraphrases.is_empty());
    assert!(rubric.incorrect_patterns.is_empty());
    assert!(rubric.dimension_weights.is_empty());
}

#[test]
fn deterministic_grade_keyword_matching() {
    let rubric = GradingRubric {
        required_keywords: vec!["rust".into(), "safe".into(), "fast".into()],
        ..Default::default()
    };
    let scores = deterministic_grade(&rubric, "Rust is safe and fast", &["factual_accuracy"]);
    assert!(scores.contains_key("factual_accuracy"));
    let score = scores["factual_accuracy"].score;
    assert!((score - 1.0).abs() < 0.001);
}

#[test]
fn deterministic_grade_partial_match() {
    let rubric = GradingRubric {
        required_keywords: vec!["alpha".into(), "beta".into(), "gamma".into(), "delta".into()],
        ..Default::default()
    };
    let scores = deterministic_grade(&rubric, "alpha and beta present", &["factual_accuracy"]);
    let score = scores["factual_accuracy"].score;
    assert!((score - 0.5).abs() < 0.001);
}

#[test]
fn deterministic_grade_paraphrase_bonus() {
    let rubric = GradingRubric {
        required_keywords: vec!["original".into()],
        acceptable_paraphrases: vec!["rephrased".into()],
        ..Default::default()
    };
    let scores =
        deterministic_grade(&rubric, "rephrased version", &["factual_accuracy", "specificity"]);
    let fa = scores["factual_accuracy"].score;
    assert!((fa - 0.25).abs() < 0.001);
}

#[test]
fn deterministic_grade_incorrect_pattern_blocks() {
    let rubric = GradingRubric {
        required_keywords: vec!["correct".into()],
        incorrect_patterns: vec!["wrong".into()],
        ..Default::default()
    };
    let scores = deterministic_grade(&rubric, "this is wrong", &["factual_accuracy"]);
    assert!((scores["factual_accuracy"].score).abs() < f64::EPSILON);
}

#[test]
fn deterministic_grade_incorrect_pattern_with_correct_present() {
    let rubric = GradingRubric {
        required_keywords: vec!["correct".into()],
        incorrect_patterns: vec!["wrong".into()],
        ..Default::default()
    };
    let scores = deterministic_grade(&rubric, "correct and wrong", &["factual_accuracy"]);
    let fa = scores["factual_accuracy"].score;
    assert!(fa > 0.0, "Should not be zeroed when correct keyword present");
}

#[test]
fn deterministic_grade_skips_non_deterministic_dims() {
    let rubric = GradingRubric {
        required_keywords: vec!["test".into()],
        ..Default::default()
    };
    let scores = deterministic_grade(
        &rubric,
        "test",
        &["temporal_awareness", "source_attribution"],
    );
    assert!(scores.is_empty());
}

#[test]
fn config_validates() {
    let config = LongHorizonConfig::default();
    assert!(config.validate().is_ok());

    let bad = LongHorizonConfig {
        num_turns: 0,
        ..Default::default()
    };
    assert!(bad.validate().is_err());

    let bad = LongHorizonConfig {
        num_questions: 0,
        ..Default::default()
    };
    assert!(bad.validate().is_err());
}

#[test]
fn multi_vote_single() {
    let grades = vec![vec![
        DimensionScore::new("factual_accuracy", 0.8, "good"),
        DimensionScore::new("specificity", 0.6, "ok"),
    ]];
    let result = multi_vote_grade(grades, &["factual_accuracy", "specificity"]);
    assert_eq!(result.len(), 2);
    assert!((result[0].score - 0.8).abs() < 0.001);
}

#[test]
fn multi_vote_median() {
    let grades = vec![
        vec![DimensionScore::new("factual_accuracy", 0.5, "low")],
        vec![DimensionScore::new("factual_accuracy", 0.9, "high")],
        vec![DimensionScore::new("factual_accuracy", 0.7, "mid")],
    ];
    let result = multi_vote_grade(grades, &["factual_accuracy"]);
    assert_eq!(result.len(), 1);
    assert!((result[0].score - 0.7).abs() < 0.001);
}

#[test]
fn multi_vote_empty() {
    let result = multi_vote_grade(vec![], &["factual_accuracy"]);
    assert_eq!(result.len(), 1);
    assert!((result[0].score).abs() < f64::EPSILON);
}

#[test]
fn long_horizon_question_serde() {
    let q = LongHorizonQuestion {
        question_id: "LH-001".into(),
        text: "What happened?".into(),
        category: "factual".into(),
        expected_answer: "The event occurred".into(),
        rubric: Some(GradingRubric {
            required_keywords: vec!["event".into()],
            ..Default::default()
        }),
    };
    let json = serde_json::to_string(&q).unwrap();
    let q2: LongHorizonQuestion = serde_json::from_str(&json).unwrap();
    assert_eq!(q.question_id, q2.question_id);
    assert!(q2.rubric.is_some());
}

#[test]
fn report_serde_roundtrip() {
    let report = LongHorizonReport {
        num_turns: 100,
        num_questions: 10,
        total_facts_delivered: 50,
        learning_time_s: 1.0,
        questioning_time_s: 0.5,
        grading_time_s: 0.2,
        overall_score: 0.75,
        category_breakdown: vec![],
        results: vec![],
        memory_stats: HashMap::new(),
    };
    let json = serde_json::to_string(&report).unwrap();
    let report2: LongHorizonReport = serde_json::from_str(&json).unwrap();
    assert!((report2.overall_score - 0.75).abs() < 0.001);
}

#[test]
fn all_dimensions_constant() {
    assert_eq!(ALL_DIMENSIONS.len(), 5);
    assert!(ALL_DIMENSIONS.contains(&"factual_accuracy"));
    assert!(ALL_DIMENSIONS.contains(&"confidence_calibration"));
}

#[test]
fn deterministic_dims_subset_of_all() {
    for d in DETERMINISTIC_DIMENSIONS {
        assert!(ALL_DIMENSIONS.contains(d));
    }
}

#[test]
fn llm_dims_subset_of_all() {
    for d in LLM_ONLY_DIMENSIONS {
        assert!(ALL_DIMENSIONS.contains(d));
    }
}
