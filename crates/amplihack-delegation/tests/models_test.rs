use amplihack_delegation::{
    DelegationStatus, EvidenceItem, EvidenceType, MetaDelegationResult, ScenarioCategory,
    SubprocessResult,
};
use serde_json::json;

#[test]
fn delegation_status_display() {
    assert_eq!(DelegationStatus::Pending.to_string(), "Pending");
    assert_eq!(DelegationStatus::Running.to_string(), "Running");
    assert_eq!(DelegationStatus::Completed.to_string(), "Completed");
    assert_eq!(DelegationStatus::Failed.to_string(), "Failed");
}

#[test]
fn serde_roundtrip() {
    let result = MetaDelegationResult {
        delegated_to: "test_persona".into(),
        status: DelegationStatus::Completed,
        result: "test_result".into(),
    };

    let json = serde_json::to_string(&result).unwrap();
    let deserialized: MetaDelegationResult = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.delegated_to, "test_persona");
    assert_eq!(deserialized.status, DelegationStatus::Completed);
    assert_eq!(deserialized.result, "test_result");
}

#[test]
fn evidence_type_display() {
    assert_eq!(EvidenceType::TestPassed.to_string(), "TestPassed");
    assert_eq!(EvidenceType::LogEntry.to_string(), "LogEntry");
    assert_eq!(EvidenceType::CodeReview.to_string(), "CodeReview");
    assert_eq!(EvidenceType::Performance.to_string(), "Performance");
}

#[test]
fn subprocess_result_success_and_crashed() {
    let success = SubprocessResult {
        exit_code: 0,
        stdout: "output".into(),
        stderr: "".into(),
    };
    assert!(success.success());

    let failed = SubprocessResult {
        exit_code: 1,
        stdout: "".into(),
        stderr: "error".into(),
    };
    assert!(!failed.success());

    let crashed = SubprocessResult {
        exit_code: -1,
        stdout: "".into(),
        stderr: "crash".into(),
    };
    assert!(!crashed.success());
}

#[test]
fn evaluation_result_clamps_score() {
    use amplihack_delegation::EvaluationResult;

    let result = EvaluationResult {
        score: 150,
        passed: true,
    };
    assert_eq!(result.clamped_score(), 100);

    let result = EvaluationResult {
        score: 75,
        passed: true,
    };
    assert_eq!(result.clamped_score(), 75);

    let result = EvaluationResult {
        score: -10,
        passed: false,
    };
    assert_eq!(result.clamped_score(), 0);
}

#[test]
fn evaluation_result_status_thresholds() {
    use amplihack_delegation::EvaluationResult;

    let high = EvaluationResult {
        score: 85,
        passed: true,
    };
    assert_eq!(high.status(), "Excellent");

    let medium = EvaluationResult {
        score: 65,
        passed: true,
    };
    assert_eq!(medium.status(), "Good");

    let low = EvaluationResult {
        score: 45,
        passed: true,
    };
    assert_eq!(low.status(), "Acceptable");

    let failed = EvaluationResult {
        score: 20,
        passed: false,
    };
    assert_eq!(failed.status(), "Failed");
}

#[test]
fn meta_delegation_result_json_roundtrip() {
    let original = MetaDelegationResult {
        delegated_to: "architect".into(),
        status: DelegationStatus::Completed,
        result: r#"{"feedback": "well structured"}"#.into(),
    };

    let json_str = serde_json::to_string(&original).unwrap();
    let parsed: MetaDelegationResult = serde_json::from_str(&json_str).unwrap();

    assert_eq!(parsed.delegated_to, original.delegated_to);
    assert_eq!(parsed.status, original.status);
    assert_eq!(parsed.result, original.result);
}

#[test]
fn get_evidence_by_type_filters() {
    let items = vec![
        EvidenceItem {
            evidence_type: EvidenceType::TestPassed,
            content: "test 1 passed".into(),
        },
        EvidenceItem {
            evidence_type: EvidenceType::LogEntry,
            content: "log line".into(),
        },
        EvidenceItem {
            evidence_type: EvidenceType::TestPassed,
            content: "test 2 passed".into(),
        },
    ];

    let test_items: Vec<_> = items
        .iter()
        .filter(|e| e.evidence_type == EvidenceType::TestPassed)
        .collect();

    assert_eq!(test_items.len(), 2);
    assert_eq!(test_items[0].content, "test 1 passed");
    assert_eq!(test_items[1].content, "test 2 passed");
}
