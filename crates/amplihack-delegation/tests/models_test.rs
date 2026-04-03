use std::collections::HashMap;

use amplihack_delegation::{
    DelegationStatus, EvaluationResult, EvidenceItem, EvidenceType, MetaDelegationResult,
    ScenarioCategory, SubprocessResult,
};
use chrono::Utc;

#[test]
fn delegation_status_display() {
    assert_eq!(DelegationStatus::Success.to_string(), "SUCCESS");
    assert_eq!(DelegationStatus::Partial.to_string(), "PARTIAL");
    assert_eq!(DelegationStatus::Failure.to_string(), "FAILURE");
}

#[test]
fn delegation_status_serde() {
    let json = serde_json::to_string(&DelegationStatus::Success).unwrap();
    assert_eq!(json, r#""SUCCESS""#);
    let parsed: DelegationStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, DelegationStatus::Success);
}

#[test]
fn evidence_type_display() {
    assert_eq!(EvidenceType::CodeFile.to_string(), "code_file");
    assert_eq!(EvidenceType::TestFile.to_string(), "test_file");
    assert_eq!(EvidenceType::Documentation.to_string(), "documentation");
    assert_eq!(EvidenceType::ExecutionLog.to_string(), "execution_log");
}

fn make_evidence(ty: EvidenceType, path: &str, content: &str) -> EvidenceItem {
    EvidenceItem {
        evidence_type: ty,
        path: path.into(),
        content: content.into(),
        excerpt: content.chars().take(40).collect(),
        size_bytes: content.len() as u64,
        timestamp: Utc::now(),
        metadata: HashMap::new(),
    }
}

#[test]
fn subprocess_result_success_and_crashed() {
    let success = SubprocessResult {
        exit_code: 0,
        stdout: "output".into(),
        stderr: String::new(),
        duration_secs: 1.0,
        subprocess_pid: 1234,
        timed_out: false,
        orphans_cleaned: 0,
    };
    assert!(success.success());
    assert!(!success.crashed());

    let failed = SubprocessResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: "error".into(),
        duration_secs: 0.5,
        subprocess_pid: 1235,
        timed_out: false,
        orphans_cleaned: 0,
    };
    assert!(!failed.success());

    let crashed = SubprocessResult {
        exit_code: -1,
        stdout: String::new(),
        stderr: "crash".into(),
        duration_secs: 0.1,
        subprocess_pid: 1236,
        timed_out: false,
        orphans_cleaned: 0,
    };
    assert!(!crashed.success());
    assert!(crashed.crashed());
}

#[test]
fn subprocess_result_timeout_is_not_success() {
    let timed_out = SubprocessResult {
        exit_code: 0,
        stdout: "partial".into(),
        stderr: String::new(),
        duration_secs: 30.0,
        subprocess_pid: 9999,
        timed_out: true,
        orphans_cleaned: 0,
    };
    assert!(!timed_out.success());
}

#[test]
fn evaluation_result_status_thresholds() {
    let high = EvaluationResult::new(85, "good".into(), vec!["r1".into()], vec![], 0);
    assert_eq!(high.status(), DelegationStatus::Success);

    let mid = EvaluationResult::new(65, "ok".into(), vec![], vec!["r2".into()], 0);
    assert_eq!(mid.status(), DelegationStatus::Partial);

    let low = EvaluationResult::new(30, "bad".into(), vec![], vec!["r3".into()], 0);
    assert_eq!(low.status(), DelegationStatus::Failure);
}

#[test]
fn evaluation_result_clamps_score() {
    let clamped = EvaluationResult::new(200, "over".into(), vec![], vec![], 0);
    assert_eq!(clamped.score, 100);
}

#[test]
fn meta_delegation_result_json_roundtrip() {
    let original = MetaDelegationResult {
        status: DelegationStatus::Success,
        success_score: 90,
        evidence: vec![make_evidence(
            EvidenceType::CodeFile,
            "src/main.rs",
            "fn main() {}",
        )],
        execution_log: "all good".into(),
        duration_secs: 5.0,
        persona_used: "architect".into(),
        platform_used: "claude_code".into(),
        failure_reason: None,
        partial_completion_notes: None,
        subprocess_pid: Some(4321),
        test_scenarios: None,
    };

    let json_str = original.to_json().unwrap();
    let parsed = MetaDelegationResult::from_json(&json_str).unwrap();

    assert_eq!(parsed.status, DelegationStatus::Success);
    assert_eq!(parsed.success_score, 90);
    assert_eq!(parsed.persona_used, "architect");
    assert_eq!(parsed.evidence.len(), 1);
    assert_eq!(parsed.evidence[0].path, "src/main.rs");
}

#[test]
fn get_evidence_by_type_filters() {
    let result = MetaDelegationResult {
        status: DelegationStatus::Success,
        success_score: 80,
        evidence: vec![
            make_evidence(EvidenceType::CodeFile, "a.rs", "code"),
            make_evidence(EvidenceType::TestFile, "test.rs", "test"),
            make_evidence(EvidenceType::CodeFile, "b.rs", "more code"),
        ],
        execution_log: String::new(),
        duration_secs: 1.0,
        persona_used: "coder".into(),
        platform_used: "copilot".into(),
        failure_reason: None,
        partial_completion_notes: None,
        subprocess_pid: None,
        test_scenarios: None,
    };

    let code_files = result.get_evidence_by_type(&EvidenceType::CodeFile);
    assert_eq!(code_files.len(), 2);
    assert_eq!(code_files[0].path, "a.rs");
    assert_eq!(code_files[1].path, "b.rs");

    let test_files = result.get_evidence_by_type(&EvidenceType::TestFile);
    assert_eq!(test_files.len(), 1);
}

#[test]
fn scenario_category_display() {
    assert_eq!(ScenarioCategory::HappyPath.to_string(), "happy_path");
    assert_eq!(ScenarioCategory::Security.to_string(), "security");
    assert_eq!(ScenarioCategory::Performance.to_string(), "performance");
}
