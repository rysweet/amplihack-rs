//! Tests for the self-improvement loop components.

use amplihack_agent_eval::models::SelfImproveConfig;
use amplihack_agent_eval::self_improve::*;

// ── VotingResult ─────────────────────────────────────────────

#[test]
fn voting_result_all_approve() {
    let vr = VotingResult {
        patch_id: "p1".into(),
        votes: vec![Vote::Approve, Vote::Approve, Vote::Approve],
        approved: true,
    };
    assert_eq!(vr.approval_count(), 3);
    assert_eq!(vr.rejection_count(), 0);
    assert!(vr.has_majority());
}

#[test]
fn voting_result_all_reject() {
    let vr = VotingResult {
        patch_id: "p1".into(),
        votes: vec![Vote::Reject, Vote::Reject, Vote::Reject],
        approved: false,
    };
    assert_eq!(vr.approval_count(), 0);
    assert_eq!(vr.rejection_count(), 3);
    assert!(!vr.has_majority());
}

#[test]
fn voting_result_majority_approve() {
    let vr = VotingResult {
        patch_id: "p1".into(),
        votes: vec![Vote::Approve, Vote::Approve, Vote::Reject],
        approved: true,
    };
    assert!(vr.has_majority());
}

#[test]
fn voting_result_majority_reject() {
    let vr = VotingResult {
        patch_id: "p1".into(),
        votes: vec![Vote::Approve, Vote::Reject, Vote::Reject],
        approved: false,
    };
    assert!(!vr.has_majority());
}

#[test]
fn voting_result_with_abstain() {
    let vr = VotingResult {
        patch_id: "p1".into(),
        votes: vec![Vote::Approve, Vote::Abstain, Vote::Abstain],
        approved: true,
    };
    assert!(vr.has_majority());
    assert_eq!(vr.approval_count(), 1);
}

#[test]
fn voting_result_all_abstain_no_majority() {
    let vr = VotingResult {
        patch_id: "p1".into(),
        votes: vec![Vote::Abstain, Vote::Abstain],
        approved: false,
    };
    assert!(!vr.has_majority());
}

#[test]
fn voting_result_empty_votes_no_majority() {
    let vr = VotingResult {
        patch_id: "p1".into(),
        votes: vec![],
        approved: false,
    };
    assert!(!vr.has_majority());
    assert_eq!(vr.approval_count(), 0);
}

// ── ErrorAnalyzer ────────────────────────────────────────────

#[test]
fn error_analyzer_construction() {
    let _analyzer = ErrorAnalyzer::new();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn error_analyzer_analyze_panics_todo() {
    let analyzer = ErrorAnalyzer::new();
    let failures = vec![
        ("test1".to_string(), "assertion failed".to_string()),
        ("test2".to_string(), "timeout".to_string()),
    ];
    let _ = analyzer.analyze(&failures);
}

// ── PatchProposer ────────────────────────────────────────────

#[test]
fn patch_proposer_construction() {
    let _proposer = PatchProposer::new();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn patch_proposer_propose_panics_todo() {
    let proposer = PatchProposer::new();
    let analyses = vec![FailureAnalysis {
        test_id: "t1".into(),
        error_category: "logic".into(),
        root_cause: "wrong comparison".into(),
        suggested_fix: "use >= instead of >".into(),
    }];
    let _ = proposer.propose(&analyses);
}

// ── ReviewerVoting ───────────────────────────────────────────

#[test]
fn reviewer_voting_valid_construction() {
    let rv = ReviewerVoting::new(3);
    assert!(rv.is_ok());
    assert_eq!(rv.unwrap().reviewer_count(), 3);
}

#[test]
fn reviewer_voting_rejects_zero() {
    assert!(ReviewerVoting::new(0).is_err());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn reviewer_voting_vote_panics_todo() {
    let rv = ReviewerVoting::new(3).unwrap();
    let patch = Patch {
        id: "p1".into(),
        description: "Fix comparison".into(),
        diff: "- x > y\n+ x >= y".into(),
        confidence: 0.9,
    };
    let _ = rv.vote(&patch);
}

// ── SelfImproveRunner ────────────────────────────────────────

#[test]
fn self_improve_runner_valid_construction() {
    let config = SelfImproveConfig::default();
    let runner = SelfImproveRunner::new(config);
    assert!(runner.is_ok());
}

#[test]
fn self_improve_runner_rejects_invalid_config() {
    let mut config = SelfImproveConfig::default();
    config.reviewer_count = 0;
    assert!(SelfImproveRunner::new(config).is_err());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn self_improve_runner_iteration_panics_todo() {
    let runner = SelfImproveRunner::new(SelfImproveConfig::default()).unwrap();
    let _ = runner.run_iteration(1);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn self_improve_runner_run_panics_todo() {
    let runner = SelfImproveRunner::new(SelfImproveConfig::default()).unwrap();
    let _ = runner.run();
}

#[test]
fn self_improve_runner_config_access() {
    let runner = SelfImproveRunner::new(SelfImproveConfig::default()).unwrap();
    assert_eq!(runner.config().max_iterations, 5);
}
