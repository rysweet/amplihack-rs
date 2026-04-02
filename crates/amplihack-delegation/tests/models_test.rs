use amplihack_delegation::models::*;
use chrono::Utc;

#[test]
fn delegation_status_display() {
    assert_eq!(DelegationStatus::Success.to_string(), "SUCCESS");
    assert_eq!(DelegationStatus::Partial.to_string(), "PARTIAL");
    assert_eq!(DelegationStatus::Failure.to_string(), "FAILURE");
}

#[test]
fn delegation_status_serde_roundtrip() {
    let json = serde_json::to_string(&DelegationStatus::Success).unwrap();
    assert_eq!(json, r#""SUCCESS""#);
    let back: DelegationStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back, DelegationStatus::Success);
}

#[test]
fn evidence_type_display() {
    assert_eq!(EvidenceType::CodeFile.to_string(), "code_file");
    assert_eq!(
        EvidenceType::ArchitectureDoc.to_string(),
        "architecture_doc"
    );
}

#[test]
fn subprocess_result_success_and_crashed() {
    let ok = SubprocessResult {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
        duration_secs: 1.0,
        subprocess_pid: 42,
        timed_out: false,
        orphans_cleaned: 0,
    };
    assert!(ok.success());
    assert!(!ok.crashed());

    let timed_out = SubprocessResult {
        timed_out: true,
        ..ok.clone()
    };
    assert!(!timed_out.success());

    let crashed = SubprocessResult {
        exit_code: -9,
        ..ok
    };
    assert!(crashed.crashed());
}

#[test]
fn evaluation_result_clamps_score() {
    let r = EvaluationResult::new(150, String::new(), vec![], vec![], 0);
    assert_eq!(r.score, 100);
}

#[test]
fn evaluation_result_status_thresholds() {
    assert_eq!(
        EvaluationResult::new(80, String::new(), vec![], vec![], 0).status(),
        DelegationStatus::Success
    );
    assert_eq!(
        EvaluationResult::new(50, String::new(), vec![], vec![], 0).status(),
        DelegationStatus::Partial
    );
    assert_eq!(
        EvaluationResult::new(49, String::new(), vec![], vec![], 0).status(),
        DelegationStatus::Failure
    );
}

#[test]
fn meta_delegation_result_json_roundtrip() {
    let result = MetaDelegationResult {
        status: DelegationStatus::Success,
        success_score: 95,
        evidence: vec![],
        execution_log: "all good".into(),
        duration_secs: 10.5,
        persona_used: "guide".into(),
        platform_used: "claude-code".into(),
        failure_reason: None,
        partial_completion_notes: None,
        subprocess_pid: Some(1234),
        test_scenarios: None,
    };
    let json = result.to_json().unwrap();
    let back = MetaDelegationResult::from_json(&json).unwrap();
    assert_eq!(back.success_score, 95);
    assert_eq!(back.status, DelegationStatus::Success);
}

#[test]
fn get_evidence_by_type_filters() {
    let item = EvidenceItem {
        evidence_type: EvidenceType::TestFile,
        path: "test_main.py".into(),
        content: String::new(),
        excerpt: String::new(),
        size_bytes: 100,
        timestamp: Utc::now(),
        metadata: Default::default(),
    };
    let result = MetaDelegationResult {
        status: DelegationStatus::Success,
        success_score: 90,
        evidence: vec![item],
        execution_log: String::new(),
        duration_secs: 1.0,
        persona_used: "guide".into(),
        platform_used: "claude-code".into(),
        failure_reason: None,
        partial_completion_notes: None,
        subprocess_pid: None,
        test_scenarios: None,
    };
    assert_eq!(
        result.get_evidence_by_type(&EvidenceType::TestFile).len(),
        1
    );
    assert_eq!(
        result.get_evidence_by_type(&EvidenceType::CodeFile).len(),
        0
    );
}
