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
fn error_analyzer_classifies_unknown_failures() {
    let analyzer = ErrorAnalyzer::new();
    let failures = vec![
        ("test1".to_string(), "assertion failed".to_string()),
        ("test2".to_string(), "timeout".to_string()),
    ];
    let results = analyzer.analyze(&failures).unwrap();
    assert_eq!(results.len(), 2);
    // Neither matches any keyword category → both "unknown"
    for r in &results {
        assert_eq!(r.error_category, "unknown");
        assert!(!r.suggested_fix.is_empty());
    }
}

#[test]
fn error_analyzer_classifies_retrieval_failure() {
    let analyzer = ErrorAnalyzer::new();
    let failures = vec![("t1".to_string(), "document not found in index".to_string())];
    let results = analyzer.analyze(&failures).unwrap();
    assert_eq!(results[0].error_category, "retrieval_insufficient");
}

#[test]
fn error_analyzer_classifies_temporal_failure() {
    let analyzer = ErrorAnalyzer::new();
    let failures = vec![("t1".to_string(), "wrong order of events".to_string())];
    let results = analyzer.analyze(&failures).unwrap();
    assert_eq!(results[0].error_category, "temporal_ordering_wrong");
}

#[test]
fn error_analyzer_empty_input_returns_empty() {
    let analyzer = ErrorAnalyzer::new();
    let results = analyzer.analyze(&[]).unwrap();
    assert!(results.is_empty());
}

// ── PatchProposer ────────────────────────────────────────────

#[test]
fn patch_proposer_construction() {
    let _proposer = PatchProposer::new();
}

#[test]
fn patch_proposer_creates_patches_with_confidence() {
    let proposer = PatchProposer::new();
    let analyses = vec![FailureAnalysis {
        test_id: "t1".into(),
        error_category: "retrieval_insufficient".into(),
        root_cause: "not found".into(),
        suggested_fix: "improve retrieval".into(),
    }];
    let patches = proposer.propose(&analyses).unwrap();
    assert_eq!(patches.len(), 1);
    assert!(patches[0].id.contains("t1"));
    // retrieval_insufficient → confidence 0.7
    assert!((patches[0].confidence - 0.7).abs() < f64::EPSILON);
    assert!(!patches[0].diff.is_empty());
}

#[test]
fn patch_proposer_unknown_category_gets_low_confidence() {
    let proposer = PatchProposer::new();
    let analyses = vec![FailureAnalysis {
        test_id: "t1".into(),
        error_category: "unknown".into(),
        root_cause: "unclassified".into(),
        suggested_fix: "manual review".into(),
    }];
    let patches = proposer.propose(&analyses).unwrap();
    assert!((patches[0].confidence - 0.3).abs() < f64::EPSILON);
}

#[test]
fn patch_proposer_empty_input_returns_empty() {
    let proposer = PatchProposer::new();
    let patches = proposer.propose(&[]).unwrap();
    assert!(patches.is_empty());
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
fn reviewer_voting_approves_high_confidence_patch() {
    let rv = ReviewerVoting::new(3).unwrap();
    let patch = Patch {
        id: "p1".into(),
        description: "Fix comparison".into(),
        diff: "- x > y\n+ x >= y".into(),
        confidence: 0.9,
    };
    let result = rv.vote(&patch).unwrap();
    assert!(result.approved);
    assert_eq!(result.votes.len(), 3);
    // confidence 0.9 exceeds all thresholds → all approve
    assert_eq!(result.approval_count(), 3);
}

#[test]
fn reviewer_voting_rejects_low_confidence_patch() {
    let rv = ReviewerVoting::new(3).unwrap();
    let patch = Patch {
        id: "p2".into(),
        description: "Risky fix".into(),
        diff: "...".into(),
        confidence: 0.3,
    };
    let result = rv.vote(&patch).unwrap();
    // 0.3 < all thresholds (0.5, 0.6, 0.4) → quality rejects, regression rejects, simplicity abstains
    assert!(!result.approved);
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
fn self_improve_runner_iteration_returns_result() {
    let runner = SelfImproveRunner::new(SelfImproveConfig::default()).unwrap();
    let result = runner.run_iteration(1).unwrap();
    assert_eq!(result.iteration, 1);
    assert_eq!(result.patches_proposed, 2); // two simulated failures
    // auto_apply_patches is false by default → no patches applied
    assert_eq!(result.patches_applied, 0);
    assert!(!result.improved);
    assert!(result.score_after > result.score_before);
}

#[test]
fn self_improve_runner_iteration_with_auto_apply() {
    let config = SelfImproveConfig {
        auto_apply_patches: true,
        ..SelfImproveConfig::default()
    };
    let runner = SelfImproveRunner::new(config).unwrap();
    let result = runner.run_iteration(0).unwrap();
    // Both patches have high enough confidence to be approved and auto-applied
    assert_eq!(result.patches_applied, 2);
    assert!(result.improved);
}

#[test]
fn self_improve_runner_runs_all_iterations() {
    let runner = SelfImproveRunner::new(SelfImproveConfig::default()).unwrap();
    let results = runner.run().unwrap();
    // target_score=0.8, scores don't reach it → runs all 5 iterations
    assert_eq!(results.len(), 5);
    // Scores should increase across iterations
    for (i, r) in results.iter().enumerate() {
        assert_eq!(r.iteration, i as u32);
    }
}

#[test]
fn self_improve_runner_stops_at_target_score() {
    let config = SelfImproveConfig {
        max_iterations: 10,
        target_score: 0.6,
        ..SelfImproveConfig::default()
    };
    let runner = SelfImproveRunner::new(config).unwrap();
    let results = runner.run().unwrap();
    // score_after for iteration i = 0.55 + i*0.05
    // iteration 1: 0.60 >= 0.6 → stops
    assert!(results.len() < 10);
    assert!(results.last().unwrap().score_after >= 0.6);
}

#[test]
fn self_improve_runner_config_access() {
    let runner = SelfImproveRunner::new(SelfImproveConfig::default()).unwrap();
    assert_eq!(runner.config().max_iterations, 5);
}
