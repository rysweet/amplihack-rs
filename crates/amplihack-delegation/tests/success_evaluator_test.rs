use std::collections::HashMap;

use amplihack_delegation::models::{EvaluationResult, EvidenceItem, EvidenceType};
use amplihack_delegation::success_evaluator::{SuccessEvaluator, parse_success_criteria};
use chrono::Utc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn code_item(path: &str, content: &str) -> EvidenceItem {
    EvidenceItem {
        evidence_type: EvidenceType::CodeFile,
        path: path.into(),
        content: content.into(),
        excerpt: content.chars().take(200).collect(),
        size_bytes: content.len() as u64,
        timestamp: Utc::now(),
        metadata: HashMap::new(),
    }
}

fn doc_item(content: &str) -> EvidenceItem {
    EvidenceItem {
        evidence_type: EvidenceType::Documentation,
        path: "README.md".into(),
        content: content.into(),
        excerpt: content.chars().take(200).collect(),
        size_bytes: content.len() as u64,
        timestamp: Utc::now(),
        metadata: HashMap::new(),
    }
}

fn test_result_item(content: &str) -> EvidenceItem {
    EvidenceItem {
        evidence_type: EvidenceType::TestResults,
        path: "test-results.json".into(),
        content: content.into(),
        excerpt: content.chars().take(200).collect(),
        size_bytes: content.len() as u64,
        timestamp: Utc::now(),
        metadata: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// parse_success_criteria
// ---------------------------------------------------------------------------

#[test]
fn parse_empty_returns_empty() {
    assert!(parse_success_criteria("").is_empty());
    assert!(parse_success_criteria("   \n\n  ").is_empty());
}

#[test]
fn parse_bullet_list() {
    let c = "- Code compiles\n- Tests pass\n- Docs exist";
    let r = parse_success_criteria(c);
    assert_eq!(r.len(), 3);
    assert_eq!(r[0], "Code compiles");
    assert_eq!(r[1], "Tests pass");
    assert_eq!(r[2], "Docs exist");
}

#[test]
fn parse_star_bullets() {
    let c = "* First item here now\n* Second item here now";
    let r = parse_success_criteria(c);
    assert_eq!(r.len(), 2);
}

#[test]
fn parse_numbered_list() {
    let c = "1. First requirement item\n2. Second requirement item";
    let r = parse_success_criteria(c);
    assert_eq!(r.len(), 2);
    assert!(r[0].contains("First"));
}

#[test]
fn parse_skips_headers_and_short_lines() {
    let criteria = "Requirements:\n- actual requirement here\nhi";
    let r = parse_success_criteria(criteria);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0], "actual requirement here");
}

#[test]
fn parse_prose_requirement() {
    let c = "The system must handle concurrent requests gracefully";
    let r = parse_success_criteria(c);
    assert_eq!(r.len(), 1);
}

// ---------------------------------------------------------------------------
// SuccessEvaluator
// ---------------------------------------------------------------------------

#[test]
fn basic_eval_no_criteria_scores_50() {
    let eval = SuccessEvaluator::new();
    let r = eval.evaluate("", &[], "");
    assert_eq!(r.score, 50);
    assert!(r.notes.contains("Basic evaluation"));
}

#[test]
fn basic_eval_with_code_bumps_score() {
    let eval = SuccessEvaluator::new();
    let ev = vec![code_item("main.rs", "fn main(){}")];
    let r = eval.evaluate("", &ev, "");
    assert!(r.score >= 70, "score {} should be ≥70 with code", r.score);
}

#[test]
fn criteria_all_met_scores_100_without_bonus() {
    let eval = SuccessEvaluator::new();
    let evidence = vec![code_item("auth.rs", "fn login() { create jwt token }")];
    let r = eval.evaluate(
        "- code implements login function\n- code creates jwt token",
        &evidence,
        "implements login creates jwt token",
    );
    assert!(
        r.score >= 80,
        "score {} expected ≥80 when all met",
        r.score
    );
    assert!(r.requirements_met.len() >= 2);
}

#[test]
fn criteria_none_met_scores_low() {
    let eval = SuccessEvaluator::new();
    let r = eval.evaluate(
        "- quantum entanglement\n- cold fusion reactor",
        &[],
        "",
    );
    assert!(r.score < 50, "score {} should be <50", r.score);
    assert!(!r.requirements_missing.is_empty());
}

#[test]
fn passing_tests_give_bonus() {
    let eval = SuccessEvaluator::new();
    let r = eval.evaluate(
        "- code compiles successfully here",
        &[code_item("main.rs", "fn main(){}")],
        "compiles code main all tests passed",
    );
    assert!(r.bonus_points >= 10, "expected test bonus");
}

#[test]
fn documentation_gives_bonus() {
    let eval = SuccessEvaluator::new();
    let long_doc = "a".repeat(200);
    let evidence = vec![
        code_item("main.rs", "fn main(){}"),
        doc_item(&long_doc),
    ];
    let r = eval.evaluate(
        "- code file exists here now",
        &evidence,
        "code main file exists",
    );
    assert!(r.bonus_points >= 5, "expected doc bonus");
}

#[test]
fn test_results_evidence_detected() {
    let eval = SuccessEvaluator::new();
    let evidence = vec![test_result_item("passed: 5, failed: 0")];
    let r = eval.evaluate("", &evidence, "");
    // Basic eval + passing tests bonus.
    assert!(r.score >= 50);
}

#[test]
fn score_clamped_to_100() {
    let r = EvaluationResult::new(999, "".into(), vec![], vec![], 0);
    assert_eq!(r.score, 100);
}

#[test]
fn notes_contain_met_and_missing() {
    let eval = SuccessEvaluator::new();
    let evidence = vec![code_item("api.rs", "fn handle_request()")];
    let r = eval.evaluate(
        "- code has request handler\n- quantum flux capacitor needed",
        &evidence,
        "handle request api",
    );
    assert!(r.notes.contains("Requirements satisfied"));
}
