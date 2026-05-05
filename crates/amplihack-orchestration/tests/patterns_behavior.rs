//! TDD tests for orchestration patterns — port of `patterns/*.py`.
//!
//! Behavioral parity contracts:
//!
//! ### `n_version`
//! - DEFAULT_PROFILES has 5 entries (conservative, pragmatic, minimalist,
//!   innovative, performance_focused) in that order.
//! - DEFAULT_CRITERIA has 5 entries: correctness, security, simplicity,
//!   philosophy_compliance, performance.
//! - `run_n_version` spawns N implementations + 1 reviewer = N+1 calls.
//! - When all implementations fail → `success=false`, `selected=None`.
//! - When reviewer fails → falls back to first successful version.
//! - Selection parsing: "hybrid" → "hybrid"; "version 2" + "select" → "version_2".
//!
//! ### `debate`
//! - DEFAULT_PERSPECTIVES has 5 entries.
//! - `run_debate` runs `rounds` rounds * `len(perspectives)` calls + 1 synthesis.
//! - When all Round 1 perspectives fail → `success=false`, `synthesis=None`,
//!   `confidence="NONE"`.
//! - Confidence parsing: "high" in synthesis → HIGH; "low" → LOW; default MEDIUM.
//!
//! ### `cascade`
//! - TIMEOUT_STRATEGIES has 3 entries (aggressive, balanced, patient) with
//!   primary/secondary/tertiary keys.
//! - FALLBACK_TEMPLATES has 5 entries (quality, service, freshness,
//!   completeness, accuracy).
//! - `run_cascade` rejects unknown fallback_strategy / timeout_strategy with
//!   error.
//! - Stops at first successful level; returns cascade_level + degradation.
//! - All levels failed → `success=false`, `cascade_level="failed"`.
//! - `create_custom_cascade` accepts arbitrary level definitions.
//!
//! ### `expert_panel`
//! - DEFAULT_EXPERTS has 3 entries (security, performance, simplicity).
//! - `aggregate_simple_majority`: tie defaults to REJECT (conservative).
//! - `aggregate_weighted`: weights by confidence; tie → REJECT.
//! - `aggregate_unanimous`: requires ALL non-abstain votes APPROVE.
//! - Quorum: non_abstain_votes >= quorum.
//! - `generate_dissent_report` returns None when no dissent, else captures
//!   dissent experts and rationales.
//! - Vote parsing: APPROVE/REJECT/ABSTAIN; invalid → ABSTAIN.
//! - Confidence parsing: float, clamped to [0.0, 1.0]; invalid → 0.5.

use std::sync::Arc;
use std::time::Duration;

use amplihack_orchestration::claude_process::{MockProcessRunner, ProcessResult, ProcessRunner};
use amplihack_orchestration::patterns::{
    cascade::{
        CascadeLevel, CustomLevel, FALLBACK_TEMPLATES, TIMEOUT_STRATEGIES, create_custom_cascade,
        run_cascade,
    },
    debate::{Confidence, DEFAULT_PERSPECTIVES, run_debate},
    expert_panel::{
        AggregationMethod, ExpertReview, VoteChoice, aggregate_simple_majority,
        aggregate_unanimous, aggregate_weighted, generate_dissent_report,
        roles::DEFAULT_EXPERTS,
        run_expert_panel,
        scoring::{extract_list_items, extract_scores, extract_section},
    },
    n_version::{DEFAULT_CRITERIA, DEFAULT_PROFILES, run_n_version},
};

// ----- n_version -----

#[test]
fn n_version_default_profiles_match_python() {
    assert_eq!(DEFAULT_PROFILES.len(), 5);
    assert_eq!(DEFAULT_PROFILES[0].name, "conservative");
    assert_eq!(DEFAULT_PROFILES[1].name, "pragmatic");
    assert_eq!(DEFAULT_PROFILES[2].name, "minimalist");
    assert_eq!(DEFAULT_PROFILES[3].name, "innovative");
    assert_eq!(DEFAULT_PROFILES[4].name, "performance_focused");
}

#[test]
fn n_version_default_criteria_match_python() {
    assert_eq!(
        DEFAULT_CRITERIA,
        &[
            "correctness",
            "security",
            "simplicity",
            "philosophy_compliance",
            "performance",
        ]
    );
}

#[tokio::test]
async fn n_version_runs_n_implementations_plus_reviewer() {
    let mock = Arc::new(MockProcessRunner::new());
    // Match any prompt with substring "Version 1", "Version 2", "Version 3"
    mock.expect_substring(
        "Version 1",
        ProcessResult::ok("v1".into(), "v1".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "Version 2",
        ProcessResult::ok("v2".into(), "v2".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "Version 3",
        ProcessResult::ok("v3".into(), "v3".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "reviewer agent",
        ProcessResult::ok(
            "## Selection\nversion 2\n## Rationale\nIt is the best, we select it.\n".into(),
            "rev".into(),
            Duration::ZERO,
        ),
    );

    let dir = tempfile::tempdir().unwrap();
    let result = run_n_version(
        "Implement password hashing".into(),
        3,
        None,
        Some(dir.path().to_path_buf()),
        None,
        None,
        None,
        mock.clone() as Arc<dyn ProcessRunner>,
    )
    .await;

    assert!(result.success);
    assert_eq!(result.versions.len(), 3);
    // 3 versions + 1 reviewer = 4 calls
    assert_eq!(mock.calls().len(), 4);
}

#[tokio::test]
async fn n_version_returns_failure_when_all_implementations_fail() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect_substring(
        "Version 1",
        ProcessResult::err("die".into(), "v1".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "Version 2",
        ProcessResult::err("die".into(), "v2".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "Version 3",
        ProcessResult::err("die".into(), "v3".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let result = run_n_version(
        "task".into(),
        3,
        None,
        Some(dir.path().to_path_buf()),
        None,
        None,
        None,
        mock as Arc<dyn ProcessRunner>,
    )
    .await;

    assert!(!result.success);
    assert!(result.selected.is_none());
    assert!(result.rationale.contains("All implementations failed"));
}

#[tokio::test]
async fn n_version_parses_hybrid_selection() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect_substring(
        "Version 1",
        ProcessResult::ok("v1".into(), "v1".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "Version 2",
        ProcessResult::ok("v2".into(), "v2".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "reviewer agent",
        ProcessResult::ok(
            "## Selection\nHYBRID\n## Rationale\nCombine both.\n".into(),
            "rev".into(),
            Duration::ZERO,
        ),
    );

    let dir = tempfile::tempdir().unwrap();
    let result = run_n_version(
        "task".into(),
        2,
        None,
        Some(dir.path().to_path_buf()),
        None,
        None,
        None,
        mock as Arc<dyn ProcessRunner>,
    )
    .await;

    assert_eq!(result.selected.as_deref(), Some("hybrid"));
}

// ----- debate -----

#[test]
fn debate_default_perspectives_match_python() {
    assert_eq!(DEFAULT_PERSPECTIVES.len(), 5);
    let names: Vec<_> = DEFAULT_PERSPECTIVES.iter().map(|p| p.name).collect();
    assert!(names.contains(&"security"));
    assert!(names.contains(&"performance"));
    assert!(names.contains(&"simplicity"));
    assert!(names.contains(&"maintainability"));
    assert!(names.contains(&"user_experience"));
}

#[tokio::test]
async fn debate_runs_rounds_times_perspectives_plus_synthesis() {
    let mock = Arc::new(MockProcessRunner::new());
    // perspectives default = [security, performance, simplicity]; rounds=2
    // Total = 2 * 3 + 1 synthesis = 7 calls.
    mock.expect_any(ProcessResult::ok("ok".into(), "any".into(), Duration::ZERO));

    let dir = tempfile::tempdir().unwrap();
    let result = run_debate(
        "Use Postgres or Redis?".into(),
        None,
        2,
        None,
        Some(dir.path().to_path_buf()),
        None,
        mock.clone() as Arc<dyn ProcessRunner>,
    )
    .await;

    assert!(result.success);
    assert_eq!(mock.calls().len(), 7);
    assert_eq!(result.rounds.len(), 2);
}

#[tokio::test]
async fn debate_fails_when_all_round1_perspectives_fail() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect_any(ProcessResult::err(
        "die".into(),
        "any".into(),
        Duration::ZERO,
    ));

    let dir = tempfile::tempdir().unwrap();
    let result = run_debate(
        "?".into(),
        None,
        1,
        None,
        Some(dir.path().to_path_buf()),
        None,
        mock as Arc<dyn ProcessRunner>,
    )
    .await;

    assert!(!result.success);
    assert!(result.synthesis.is_none());
    assert_eq!(result.confidence, Confidence::None);
}

// ----- cascade -----

#[test]
fn cascade_timeout_strategies_match_python() {
    assert_eq!(TIMEOUT_STRATEGIES.get("aggressive").unwrap().primary, 5);
    assert_eq!(TIMEOUT_STRATEGIES.get("balanced").unwrap().primary, 30);
    assert_eq!(TIMEOUT_STRATEGIES.get("patient").unwrap().primary, 120);
    assert_eq!(TIMEOUT_STRATEGIES.len(), 3);
}

#[test]
fn cascade_fallback_templates_match_python() {
    let names = [
        "quality",
        "service",
        "freshness",
        "completeness",
        "accuracy",
    ];
    for n in names {
        assert!(FALLBACK_TEMPLATES.contains_key(n), "missing template: {n}");
    }
    assert_eq!(FALLBACK_TEMPLATES.len(), 5);
}

#[tokio::test]
async fn cascade_returns_at_first_successful_level() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect_substring(
        "PRIMARY",
        ProcessResult::ok("yay".into(), "p".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let result = run_cascade(
        "Generate docs".into(),
        "quality".into(),
        "balanced".into(),
        None,
        Some(dir.path().to_path_buf()),
        "warning".into(),
        None,
        None,
        mock.clone() as Arc<dyn ProcessRunner>,
    )
    .await
    .unwrap();

    assert!(result.success);
    assert_eq!(result.cascade_level, CascadeLevel::Primary);
    assert_eq!(mock.calls().len(), 1);
    assert!(result.degradation.is_none());
}

#[tokio::test]
async fn cascade_falls_back_to_secondary_then_tertiary() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect_substring(
        "PRIMARY",
        ProcessResult::err("p-fail".into(), "p".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "SECONDARY",
        ProcessResult::err("s-fail".into(), "s".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "TERTIARY",
        ProcessResult::ok("rescued".into(), "t".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let result = run_cascade(
        "task".into(),
        "quality".into(),
        "aggressive".into(),
        None,
        Some(dir.path().to_path_buf()),
        "warning".into(),
        None,
        None,
        mock as Arc<dyn ProcessRunner>,
    )
    .await
    .unwrap();

    assert!(result.success);
    assert_eq!(result.cascade_level, CascadeLevel::Tertiary);
    assert!(result.degradation.is_some());
}

#[tokio::test]
async fn cascade_rejects_unknown_fallback_strategy() {
    let mock = Arc::new(MockProcessRunner::new());
    let dir = tempfile::tempdir().unwrap();
    let result = run_cascade(
        "task".into(),
        "bogus".into(),
        "balanced".into(),
        None,
        Some(dir.path().to_path_buf()),
        "warning".into(),
        None,
        None,
        mock as Arc<dyn ProcessRunner>,
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn cascade_rejects_unknown_timeout_strategy() {
    let mock = Arc::new(MockProcessRunner::new());
    let dir = tempfile::tempdir().unwrap();
    let result = run_cascade(
        "task".into(),
        "quality".into(),
        "bogus".into(),
        None,
        Some(dir.path().to_path_buf()),
        "warning".into(),
        None,
        None,
        mock as Arc<dyn ProcessRunner>,
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn create_custom_cascade_supports_arbitrary_levels() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect_substring(
        "COMPREHENSIVE",
        ProcessResult::err("nope".into(), "x".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "QUICK",
        ProcessResult::ok("done".into(), "q".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let levels = vec![
        CustomLevel {
            name: "comprehensive".into(),
            timeout: Duration::from_secs(60),
            constraint: "full analysis".into(),
            model: None,
        },
        CustomLevel {
            name: "quick".into(),
            timeout: Duration::from_secs(10),
            constraint: "quick scan".into(),
            model: None,
        },
    ];

    let result = create_custom_cascade(
        "Analyze code".into(),
        levels,
        Some(dir.path().to_path_buf()),
        "warning".into(),
        mock as Arc<dyn ProcessRunner>,
    )
    .await
    .unwrap();

    assert!(result.success);
    assert!(
        result.degradation.is_some(),
        "should report degradation when not first level"
    );
}

// ----- expert_panel -----

#[test]
fn expert_panel_default_experts_match_python() {
    assert_eq!(DEFAULT_EXPERTS.len(), 3);
    assert_eq!(DEFAULT_EXPERTS[0].domain, "security");
    assert_eq!(DEFAULT_EXPERTS[1].domain, "performance");
    assert_eq!(DEFAULT_EXPERTS[2].domain, "simplicity");
}

fn make_review(domain: &str, vote: VoteChoice, confidence: f32) -> ExpertReview {
    ExpertReview {
        expert_id: format!("{domain}-expert"),
        domain: domain.into(),
        analysis: String::new(),
        strengths: vec![],
        weaknesses: vec![],
        domain_scores: Default::default(),
        vote,
        confidence,
        vote_rationale: String::new(),
        review_duration: Duration::ZERO,
    }
}

#[test]
fn aggregate_simple_majority_majority_approves() {
    let reviews = vec![
        make_review("security", VoteChoice::Approve, 0.9),
        make_review("performance", VoteChoice::Approve, 0.8),
        make_review("simplicity", VoteChoice::Reject, 0.7),
    ];
    let d = aggregate_simple_majority(&reviews, 3);
    assert_eq!(d.decision, VoteChoice::Approve);
    assert_eq!(d.approve_votes, 2);
    assert_eq!(d.reject_votes, 1);
    assert!(d.quorum_met);
    assert_eq!(d.dissenting_opinions.len(), 1);
}

#[test]
fn aggregate_simple_majority_tie_defaults_to_reject() {
    let reviews = vec![
        make_review("a", VoteChoice::Approve, 0.5),
        make_review("b", VoteChoice::Reject, 0.5),
    ];
    let d = aggregate_simple_majority(&reviews, 2);
    assert_eq!(d.decision, VoteChoice::Reject);
}

#[test]
fn aggregate_simple_majority_quorum_not_met_when_too_few() {
    let reviews = vec![
        make_review("a", VoteChoice::Approve, 1.0),
        make_review("b", VoteChoice::Abstain, 1.0),
        make_review("c", VoteChoice::Abstain, 1.0),
    ];
    let d = aggregate_simple_majority(&reviews, 3);
    assert!(
        !d.quorum_met,
        "1 non-abstain vote should not satisfy quorum=3"
    );
}

#[test]
fn aggregate_simple_majority_unanimous_consensus_type() {
    let reviews = vec![
        make_review("a", VoteChoice::Approve, 0.9),
        make_review("b", VoteChoice::Approve, 0.9),
        make_review("c", VoteChoice::Approve, 0.9),
    ];
    let d = aggregate_simple_majority(&reviews, 3);
    assert_eq!(d.consensus_type, "unanimous");
    assert_eq!(d.agreement_percentage, 100.0);
}

#[test]
fn aggregate_weighted_uses_confidence_weights() {
    let reviews = vec![
        // 2 weak approve (0.3 + 0.3 = 0.6) vs 1 strong reject (0.9)
        make_review("a", VoteChoice::Approve, 0.3),
        make_review("b", VoteChoice::Approve, 0.3),
        make_review("c", VoteChoice::Reject, 0.9),
    ];
    let d = aggregate_weighted(&reviews, 3);
    assert_eq!(
        d.decision,
        VoteChoice::Reject,
        "strong reject (0.9) should outweigh two weak approves (0.6)"
    );
}

#[test]
fn aggregate_unanimous_requires_all_approve() {
    let reviews_all_approve = vec![
        make_review("a", VoteChoice::Approve, 0.9),
        make_review("b", VoteChoice::Approve, 0.9),
    ];
    let d = aggregate_unanimous(&reviews_all_approve, 2);
    assert_eq!(d.decision, VoteChoice::Approve);
    assert_eq!(d.consensus_type, "unanimous");

    let reviews_one_dissent = vec![
        make_review("a", VoteChoice::Approve, 0.9),
        make_review("b", VoteChoice::Reject, 0.9),
    ];
    let d2 = aggregate_unanimous(&reviews_one_dissent, 2);
    assert_eq!(d2.decision, VoteChoice::Reject);
    assert_eq!(d2.consensus_type, "not_unanimous");
}

#[test]
fn generate_dissent_report_returns_none_when_no_dissent() {
    let reviews = vec![
        make_review("a", VoteChoice::Approve, 0.9),
        make_review("b", VoteChoice::Approve, 0.9),
    ];
    let d = aggregate_simple_majority(&reviews, 2);
    assert!(generate_dissent_report(&d).is_none());
}

#[test]
fn generate_dissent_report_captures_dissenters() {
    let reviews = vec![
        make_review("a", VoteChoice::Approve, 0.9),
        make_review("b", VoteChoice::Approve, 0.9),
        make_review("c", VoteChoice::Reject, 0.8),
    ];
    let d = aggregate_simple_majority(&reviews, 3);
    let r = generate_dissent_report(&d).expect("should produce a dissent report");
    assert_eq!(r.dissent_count, 1);
    assert_eq!(r.majority_count, 2);
    assert_eq!(r.dissent_experts, vec!["c-expert"]);
}

#[test]
fn extract_section_handles_basic_markdown() {
    let text = "## Vote\nAPPROVE\n\n## Confidence\n0.85\n";
    assert_eq!(extract_section(text, "Vote"), "APPROVE");
    assert_eq!(extract_section(text, "Confidence"), "0.85");
    assert_eq!(extract_section(text, "Missing"), "");
}

#[test]
fn extract_list_items_parses_bullets() {
    let text = "## Strengths\n- Clear API\n- Fast\n* Safe\n";
    let items = extract_list_items(text, "Strengths");
    assert_eq!(items, vec!["Clear API", "Fast", "Safe"]);
}

#[test]
fn extract_scores_parses_key_value_floats() {
    let text = "## Domain Scores\n- accuracy: 0.8\n- latency: 0.6\n";
    let scores = extract_scores(text, "Domain Scores");
    assert_eq!(scores.get("accuracy"), Some(&0.8));
    assert_eq!(scores.get("latency"), Some(&0.6));
}

#[tokio::test]
async fn run_expert_panel_integrates_review_and_aggregation() {
    let mock = Arc::new(MockProcessRunner::new());
    let approve_text = "## Analysis\nfine\n## Strengths\n- A\n## Weaknesses\n- B\n## Domain Scores\n- s: 0.9\n## Vote\nAPPROVE\n## Confidence\n0.9\n## Vote Rationale\nlooks good\n";
    let reject_text = "## Analysis\nbad\n## Strengths\n- nothing\n## Weaknesses\n- everything\n## Domain Scores\n- s: 0.1\n## Vote\nREJECT\n## Confidence\n0.9\n## Vote Rationale\nbroken\n";

    mock.expect_substring(
        "YOUR EXPERTISE: SECURITY",
        ProcessResult::ok(approve_text.into(), "s".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "YOUR EXPERTISE: PERFORMANCE",
        ProcessResult::ok(approve_text.into(), "p".into(), Duration::ZERO),
    );
    mock.expect_substring(
        "YOUR EXPERTISE: SIMPLICITY",
        ProcessResult::ok(reject_text.into(), "x".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let result = run_expert_panel(
        "def hash_password(pwd): pass".into(),
        None,
        AggregationMethod::SimpleMajority,
        3,
        None,
        Some(dir.path().to_path_buf()),
        None,
        mock as Arc<dyn ProcessRunner>,
    )
    .await;

    assert!(result.success);
    let decision = result.decision.expect("decision should be present");
    assert_eq!(decision.decision, VoteChoice::Approve);
    assert_eq!(decision.approve_votes, 2);
    assert_eq!(decision.reject_votes, 1);
    assert!(result.dissent_report.is_some());
}

#[tokio::test]
async fn run_expert_panel_invalid_aggregation_returns_error_at_compile_time() {
    // The Rust API uses an enum, so invalid methods are unrepresentable.
    // This test documents that intent.
    let _ = AggregationMethod::SimpleMajority;
    let _ = AggregationMethod::Weighted;
    let _ = AggregationMethod::Unanimous;
}
