//! Tests for teaching_eval module.

use super::*;

fn make_good_result() -> TeachingResult {
    TeachingResult {
        instruction: "1. First step: understand the basics of security review.\n\n\
                      2. Second step: look for common vulnerabilities.\n\n\
                      3. Third step: check for injection attacks.\n\n\
                      4. Fourth step: review for best practices.\n\n\
                      For example, a SQL injection vulnerability looks like: \
                      SELECT * FROM users WHERE name = '$input'.\n\n\
                      **Important**: Always check for bug patterns, security issues, \
                      and naming convention violations. Use proper test coverage \
                      and refactor when needed."
            .into(),
        lesson_plan: "Lesson 1: Introduction to code review for beginner students\n\
                      Lesson 2: Security vulnerabilities\n\
                      Lesson 3: Code quality patterns\n\
                      Lesson 4: Practice exercises\n\
                      Lesson 5: Advanced topics"
            .into(),
        agent_answers: vec![
            "SQL injection occurs when user input is not properly sanitized. \
             You should use parameterized queries to prevent this type of vulnerability."
                .into(),
            "XSS vulnerabilities happen when user input is reflected in HTML without encoding. \
             Always use HTML encoding for user-generated content."
                .into(),
        ],
        student_attempt: "I found the following issues in the code:\n\
                          - Identified a SQL injection vulnerability in the login function\n\
                          - Detected an XSS issue in the comment rendering\n\
                          Summary: Two critical security findings need immediate action."
            .into(),
    }
}

fn make_empty_result() -> TeachingResult {
    TeachingResult::default()
}

#[test]
fn grade_clarity_good_instruction() {
    let result = make_good_result();
    let score = grade_clarity(&result, "code_review");
    assert!(score.score > 0.5, "Good instruction should score well: {}", score.score);
    assert!(!score.details.is_empty());
}

#[test]
fn grade_clarity_empty_instruction() {
    let result = make_empty_result();
    let score = grade_clarity(&result, "code_review");
    assert!((score.score).abs() < f64::EPSILON);
    assert!(score.details.contains("No instruction"));
}

#[test]
fn grade_clarity_short_instruction() {
    let result = TeachingResult {
        instruction: "Do security review.".into(),
        ..Default::default()
    };
    let score = grade_clarity(&result, "code_review");
    assert!(score.score < 0.5);
}

#[test]
fn grade_completeness_good() {
    let result = make_good_result();
    let score = grade_completeness(&result);
    assert!(score.score > 0.5, "Complete result should score well");
}

#[test]
fn grade_completeness_empty() {
    let result = make_empty_result();
    let score = grade_completeness(&result);
    assert!(score.score < 0.3);
}

#[test]
fn grade_student_performance_good() {
    let result = make_good_result();
    let score = grade_student_performance(&result);
    assert!(
        score.score > 0.5,
        "Good student performance should score well: {}",
        score.score
    );
}

#[test]
fn grade_student_performance_empty() {
    let result = make_empty_result();
    let score = grade_student_performance(&result);
    assert!((score.score).abs() < f64::EPSILON);
    assert!(score.details.contains("No student attempt"));
}

#[test]
fn grade_adaptivity_varied_answers() {
    let result = make_good_result();
    let score = grade_adaptivity(&result);
    assert!(score.score > 0.3);
}

#[test]
fn grade_adaptivity_no_answers() {
    let result = make_empty_result();
    let score = grade_adaptivity(&result);
    assert!(score.score < 0.3);
}

#[test]
fn grade_adaptivity_level_aware() {
    let result = TeachingResult {
        lesson_plan: "This lesson is designed for beginner students".into(),
        agent_answers: vec![
            "Answer one with detail".repeat(5),
            "Answer two with different detail".repeat(5),
        ],
        ..Default::default()
    };
    let score = grade_adaptivity(&result);
    assert!(score.score > 0.5, "Level-aware result should score well");
    assert!(score.details.contains("Level-aware"));
}

#[test]
fn evaluate_teaching_composite() {
    let result = make_good_result();
    let eval = evaluate_teaching("agent1", "code_review", "security", "beginner", &result);
    assert_eq!(eval.agent_name, "agent1");
    assert_eq!(eval.domain, "code_review");
    assert_eq!(eval.topic, "security");
    assert_eq!(eval.dimension_scores.len(), 4);
    assert!(eval.composite_score > 0.0);
    // Composite should be sum of score * weight
    let manual: f64 = eval.dimension_scores.iter().map(|d| d.score * d.weight).sum();
    assert!((eval.composite_score - manual).abs() < 0.001);
}

#[test]
fn evaluate_teaching_empty_result() {
    let result = make_empty_result();
    let eval = evaluate_teaching("agent2", "code_review", "security", "beginner", &result);
    assert!(eval.composite_score < 0.2);
}

#[test]
fn teaching_eval_result_recompute() {
    let result = make_good_result();
    let mut eval = evaluate_teaching("agent", "code_review", "security", "beginner", &result);
    let original = eval.composite_score;
    eval.dimension_scores[0].score = 1.0;
    eval.recompute_composite();
    assert!(eval.composite_score >= original);
}

#[test]
fn teaching_eval_result_to_summary() {
    let result = make_good_result();
    let eval = evaluate_teaching("agent", "code_review", "security", "beginner", &result);
    let summary = eval.to_summary();
    assert_eq!(summary["agent_name"], "agent");
    assert_eq!(summary["domain"], "code_review");
    assert!(summary["dimensions"].as_array().unwrap().len() == 4);
}

#[test]
fn default_weights_sum_to_one() {
    let weights = default_weights();
    let total: f64 = weights.values().sum();
    assert!((total - 1.0).abs() < f64::EPSILON);
}

#[test]
fn domain_terms_code_review() {
    let terms = get_domain_terms("code_review");
    assert!(!terms.is_empty());
    assert!(terms.contains(&"bug"));
    assert!(terms.contains(&"security"));
}

#[test]
fn domain_terms_unknown_domain() {
    let terms = get_domain_terms("unknown_domain");
    assert!(terms.is_empty());
}

#[test]
fn combined_score_calculation() {
    let score = combined_score(0.8, 0.6, 0.6, 0.4);
    assert!((score - 0.72).abs() < 0.001);
}

#[test]
fn combined_score_edge_cases() {
    assert!((combined_score(1.0, 1.0, 0.5, 0.5) - 1.0).abs() < f64::EPSILON);
    assert!((combined_score(0.0, 0.0, 0.5, 0.5)).abs() < f64::EPSILON);
}

#[test]
fn teaching_dimension_score_capped() {
    let ds = TeachingDimensionScore::new("clarity", 1.5, 0.25, "test");
    assert!((ds.score - 1.0).abs() < f64::EPSILON);
}

#[test]
fn teaching_result_serde() {
    let result = make_good_result();
    let json = serde_json::to_string(&result).unwrap();
    let result2: TeachingResult = serde_json::from_str(&json).unwrap();
    assert_eq!(result.instruction, result2.instruction);
    assert_eq!(result.agent_answers.len(), result2.agent_answers.len());
}

#[test]
fn teaching_eval_result_serde() {
    let result = make_good_result();
    let eval = evaluate_teaching("agent", "code_review", "security", "beginner", &result);
    let json = serde_json::to_string(&eval).unwrap();
    let eval2: TeachingEvalResult = serde_json::from_str(&json).unwrap();
    assert_eq!(eval2.agent_name, "agent");
    assert!((eval2.composite_score - eval.composite_score).abs() < 0.001);
}
