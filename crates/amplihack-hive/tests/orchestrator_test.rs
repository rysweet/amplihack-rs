use amplihack_hive::{DefaultPromotionPolicy, HiveFact, HiveMindOrchestrator, PromotionPolicy};
use chrono::Utc;

fn make_fact(concept: &str, confidence: f64) -> HiveFact {
    HiveFact {
        fact_id: format!("fact-{concept}"),
        concept: concept.to_string(),
        content: format!("{concept} content"),
        confidence,
        source_id: "agent-1".to_string(),
        tags: vec![],
        created_at: Utc::now(),
    }
}

// --- DefaultPromotionPolicy tests (REAL implementation, should pass) ---

#[test]
fn default_policy_thresholds() {
    let policy = DefaultPromotionPolicy {
        promote_threshold: 0.7,
        broadcast_threshold: 0.9,
    };
    assert!((policy.promote_threshold - 0.7).abs() < f64::EPSILON);
    assert!((policy.broadcast_threshold - 0.9).abs() < f64::EPSILON);
}

#[test]
fn policy_promotes_high_confidence() {
    let policy = DefaultPromotionPolicy {
        promote_threshold: 0.7,
        broadcast_threshold: 0.9,
    };
    let fact = make_fact("important", 0.85);
    assert!(policy.should_promote(&fact, "agent-1"));
}

#[test]
fn policy_rejects_low_confidence() {
    let policy = DefaultPromotionPolicy {
        promote_threshold: 0.7,
        broadcast_threshold: 0.9,
    };
    let fact = make_fact("weak", 0.3);
    assert!(!policy.should_promote(&fact, "agent-1"));
}

#[test]
fn policy_promotes_at_exact_threshold() {
    let policy = DefaultPromotionPolicy {
        promote_threshold: 0.7,
        broadcast_threshold: 0.9,
    };
    let fact = make_fact("borderline", 0.7);
    assert!(policy.should_promote(&fact, "agent-1"));
}

#[test]
fn policy_broadcasts_very_high() {
    let policy = DefaultPromotionPolicy {
        promote_threshold: 0.7,
        broadcast_threshold: 0.9,
    };
    let fact = make_fact("critical", 0.95);
    assert!(policy.should_broadcast(&fact));
}

#[test]
fn policy_does_not_broadcast_medium() {
    let policy = DefaultPromotionPolicy {
        promote_threshold: 0.7,
        broadcast_threshold: 0.9,
    };
    let fact = make_fact("medium", 0.75);
    assert!(!policy.should_broadcast(&fact));
}

#[test]
fn policy_broadcasts_at_exact_threshold() {
    let policy = DefaultPromotionPolicy {
        promote_threshold: 0.7,
        broadcast_threshold: 0.9,
    };
    let fact = make_fact("exact", 0.9);
    assert!(policy.should_broadcast(&fact));
}

#[test]
fn custom_policy_thresholds() {
    let policy = DefaultPromotionPolicy {
        promote_threshold: 0.5,
        broadcast_threshold: 0.6,
    };
    let fact = make_fact("easy", 0.55);
    assert!(policy.should_promote(&fact, "agent-x"));
    assert!(!policy.should_broadcast(&fact));

    let high_fact = make_fact("high", 0.65);
    assert!(policy.should_broadcast(&high_fact));
}

// --- HiveMindOrchestrator accessor tests (should pass) ---

#[test]
fn orchestrator_with_default_policy_is_constructible() {
    let _orch = HiveMindOrchestrator::with_default_policy();
}

#[test]
fn orchestrator_policy_accessible() {
    let orch = HiveMindOrchestrator::with_default_policy();
    let fact = make_fact("test", 0.95);
    assert!(orch.policy().should_promote(&fact, "agent-1"));
    assert!(orch.policy().should_broadcast(&fact));
}

#[test]
fn orchestrator_custom_policy() {
    let policy = DefaultPromotionPolicy {
        promote_threshold: 0.3,
        broadcast_threshold: 0.4,
    };
    let orch = HiveMindOrchestrator::new(Box::new(policy));
    let fact = make_fact("low", 0.35);
    assert!(orch.policy().should_promote(&fact, "agent-1"));
}

// --- HiveMindOrchestrator behavioral tests ---

#[test]
fn orchestrator_store_fact() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let id = orch
        .store_fact("rust", "systems language", 0.9, "agent-1")
        .unwrap();
    assert!(!id.is_empty());
}

#[test]
fn orchestrator_query() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    orch.store_fact("rust", "systems language", 0.9, "agent-1")
        .unwrap();
    let facts = orch.query("rust").unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
}

#[test]
fn orchestrator_promote() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let id = orch
        .store_fact("rust", "systems language", 0.9, "agent-1")
        .unwrap();
    // 0.9 >= 0.7 threshold → should promote
    let promoted = orch.promote(&id, "agent-1").unwrap();
    assert!(promoted);
}

#[test]
fn orchestrator_promote_below_threshold() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let id = orch
        .store_fact("rumor", "unverified", 0.3, "agent-1")
        .unwrap();
    // 0.3 < 0.7 threshold → should not promote
    let promoted = orch.promote(&id, "agent-1").unwrap();
    assert!(!promoted);
}

#[test]
fn orchestrator_promote_nonexistent() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let promoted = orch.promote("nonexistent-id", "agent-1").unwrap();
    assert!(!promoted);
}
