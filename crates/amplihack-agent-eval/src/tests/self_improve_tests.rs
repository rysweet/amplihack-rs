use super::*;
use crate::models::SelfImproveConfig;

#[test]
fn voting_result_counts() {
    let vr = VotingResult {
        patch_id: "p1".into(),
        votes: vec![Vote::Approve, Vote::Reject, Vote::Approve, Vote::Abstain],
    };
    assert_eq!(vr.approval_count(), 2);
    assert_eq!(vr.rejection_count(), 1);
}

#[test]
fn voting_result_majority() {
    let approved = VotingResult {
        patch_id: "p1".into(),
        votes: vec![Vote::Approve, Vote::Approve, Vote::Reject],
    };
    assert!(approved.has_majority());

    let rejected = VotingResult {
        patch_id: "p2".into(),
        votes: vec![Vote::Reject, Vote::Reject, Vote::Approve],
    };
    assert!(!rejected.has_majority());
}

#[test]
fn voting_result_all_abstain_no_majority() {
    let vr = VotingResult {
        patch_id: "p1".into(),
        votes: vec![Vote::Abstain, Vote::Abstain],
    };
    assert!(!vr.has_majority());
}

#[test]
fn error_analyzer_sorts_unknown_first() {
    let analyzer = ErrorAnalyzer::new();
    let failures = vec![
        ("t1".into(), "retrieval not found".into()),
        ("t2".into(), "something completely novel".into()),
    ];
    let analyses = analyzer.analyze(&failures).unwrap();
    assert_eq!(analyses[0].error_category, "unknown");
}

#[test]
fn error_analyzer_classifies_retrieval() {
    let analyzer = ErrorAnalyzer::new();
    let failures = vec![("t1".into(), "result not found in store".into())];
    let analyses = analyzer.analyze(&failures).unwrap();
    assert_eq!(analyses[0].error_category, "retrieval_insufficient");
}

#[test]
fn patch_proposer_produces_patches() {
    let analyses = vec![FailureAnalysis {
        test_id: "t1".into(),
        error_category: "retrieval_insufficient".into(),
        root_cause: "missing".into(),
        suggested_fix: "expand search".into(),
    }];
    let proposer = PatchProposer::new();
    let patches = proposer.propose(&analyses).unwrap();
    assert_eq!(patches.len(), 1);
    assert!((patches[0].confidence - 0.7).abs() < f64::EPSILON);
}

#[test]
fn reviewer_voting_rejects_zero_reviewers() {
    assert!(ReviewerVoting::new(0).is_err());
}

#[test]
fn reviewer_voting_high_confidence_approved() {
    let voting = ReviewerVoting::new(3).unwrap();
    let patch = Patch {
        id: "p1".into(),
        description: "fix".into(),
        diff: "diff".into(),
        confidence: 0.8,
    };
    let result = voting.vote(&patch).unwrap();
    assert!(result.approved());
}

#[test]
fn reviewer_voting_low_confidence_rejected() {
    let voting = ReviewerVoting::new(3).unwrap();
    let patch = Patch {
        id: "p1".into(),
        description: "fix".into(),
        diff: "diff".into(),
        confidence: 0.2,
    };
    let result = voting.vote(&patch).unwrap();
    assert!(!result.approved());
}

#[test]
fn self_improve_runner_rejects_invalid_config() {
    let config = SelfImproveConfig {
        max_iterations: 0,
        ..Default::default()
    };
    assert!(SelfImproveRunner::new(config).is_err());
}

#[test]
fn self_improve_runner_iteration() {
    let config = SelfImproveConfig {
        auto_apply_patches: true,
        ..Default::default()
    };
    let runner = SelfImproveRunner::new(config).unwrap();
    let result = runner.run_iteration(0).unwrap();
    assert_eq!(result.iteration, 0);
    assert!(result.patches_proposed > 0);
}

#[test]
fn self_improve_runner_loop_terminates() {
    let config = SelfImproveConfig {
        max_iterations: 3,
        target_score: 1.0, // unreachable with simulated scores, so runs all iterations
        auto_apply_patches: true,
        ..Default::default()
    };
    let runner = SelfImproveRunner::new(config).unwrap();
    let results = runner.run().unwrap();
    assert_eq!(results.len(), 3);
}

#[test]
fn self_improve_runner_stops_at_target() {
    let config = SelfImproveConfig {
        max_iterations: 10,
        target_score: 0.0, // immediately met
        auto_apply_patches: true,
        ..Default::default()
    };
    let runner = SelfImproveRunner::new(config).unwrap();
    let results = runner.run().unwrap();
    // Should stop after first iteration since score_after >= 0.0
    assert_eq!(results.len(), 1);
}
