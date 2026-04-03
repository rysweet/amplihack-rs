use amplihack_hive::{
    DefaultPromotionPolicy, GossipConfig, GossipProtocol, HiveFact, HiveMindOrchestrator,
    PromotionPolicy, make_event,
};
use chrono::Utc;
use std::collections::HashMap;

fn make_fact(concept: &str, confidence: f64) -> HiveFact {
    HiveFact {
        fact_id: format!("fact-{concept}"),
        concept: concept.to_string(),
        content: format!("{concept} content"),
        confidence,
        source_id: "agent-1".to_string(),
        tags: vec![],
        created_at: Utc::now(),
        status: "promoted".to_string(),
        metadata: HashMap::new(),
    }
}

// --- DefaultPromotionPolicy tests ---

#[test]
fn default_policy_thresholds() {
    let policy = DefaultPromotionPolicy::default();
    assert!((policy.promote_threshold - 0.7).abs() < f64::EPSILON);
    assert!((policy.broadcast_threshold - 0.9).abs() < f64::EPSILON);
    assert!((policy.gossip_threshold - 0.5).abs() < f64::EPSILON);
}

#[test]
fn policy_promotes_high_confidence() {
    let policy = DefaultPromotionPolicy::default();
    let fact = make_fact("important", 0.85);
    assert!(policy.should_promote(&fact, "agent-1"));
}

#[test]
fn policy_rejects_low_confidence() {
    let policy = DefaultPromotionPolicy::default();
    let fact = make_fact("weak", 0.3);
    assert!(!policy.should_promote(&fact, "agent-1"));
}

#[test]
fn policy_promotes_at_exact_threshold() {
    let policy = DefaultPromotionPolicy::default();
    let fact = make_fact("borderline", 0.7);
    assert!(policy.should_promote(&fact, "agent-1"));
}

#[test]
fn policy_broadcasts_very_high() {
    let policy = DefaultPromotionPolicy::default();
    let fact = make_fact("critical", 0.95);
    assert!(policy.should_broadcast(&fact));
}

#[test]
fn policy_does_not_broadcast_medium() {
    let policy = DefaultPromotionPolicy::default();
    let fact = make_fact("medium", 0.75);
    assert!(!policy.should_broadcast(&fact));
}

#[test]
fn custom_policy_thresholds() {
    let policy = DefaultPromotionPolicy {
        promote_threshold: 0.5,
        broadcast_threshold: 0.6,
        gossip_threshold: 0.3,
    };
    let fact = make_fact("easy", 0.55);
    assert!(policy.should_promote(&fact, "agent-x"));
    assert!(!policy.should_broadcast(&fact));
}

// --- HiveMindOrchestrator accessor tests ---

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
        gossip_threshold: 0.2,
    };
    let orch = HiveMindOrchestrator::new(Box::new(policy));
    let fact = make_fact("low", 0.35);
    assert!(orch.policy().should_promote(&fact, "agent-1"));
}

#[test]
fn orchestrator_with_agent_id() {
    let orch = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("test-agent".to_string());
    assert_eq!(orch.agent_id(), "test-agent");
}

// --- store_and_promote tests ---

#[test]
fn store_and_promote_high_confidence() {
    let mut orch = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("a1".to_string());
    let result = orch.store_and_promote("rust", "systems language", 0.9, "a1").unwrap();
    assert!(!result.fact_id.is_empty());
    assert!(result.promoted);
    assert!(result.broadcast);
}

#[test]
fn store_and_promote_low_confidence() {
    let mut orch = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("a1".to_string());
    let result = orch.store_and_promote("rumor", "unverified", 0.3, "a1").unwrap();
    assert!(!result.promoted);
    assert!(!result.broadcast);
}

// --- query_unified tests ---

#[test]
fn query_unified_local_only() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    orch.store_fact("rust", "systems language", 0.9, "a1").unwrap();
    let facts = orch.query_unified("rust").unwrap();
    assert_eq!(facts.len(), 1);
}

#[test]
fn query_unified_with_peers() {
    let mut peer = HiveMindOrchestrator::with_default_policy();
    peer.store_fact("rust", "memory safe", 0.85, "a2").unwrap();

    let mut orch = HiveMindOrchestrator::with_default_policy();
    orch.store_fact("rust", "systems language", 0.9, "a1").unwrap();
    orch.add_peer(peer);

    let facts = orch.query_unified("rust").unwrap();
    assert_eq!(facts.len(), 2);
    // Should be sorted by confidence descending
    assert!(facts[0].confidence >= facts[1].confidence);
}

// --- process_event tests ---

#[test]
fn process_event_incorporates_peer_fact() {
    let mut orch = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("me".to_string());
    let event = make_event("FACT_PROMOTED", "peer",
        serde_json::json!({"concept": "rust", "content": "fast", "confidence": 0.8}));
    let result = orch.process_event(&event);
    assert!(result.incorporated);
    assert!(result.fact_id.is_some());
}

#[test]
fn process_event_skips_non_fact_promoted() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let event = make_event("OTHER", "src", serde_json::json!({}));
    let result = orch.process_event(&event);
    assert!(!result.incorporated);
    assert!(result.reason.contains("not a FACT_PROMOTED"));
}

#[test]
fn process_event_skips_self() {
    let mut orch = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("me".to_string());
    let event = make_event("FACT_PROMOTED", "me",
        serde_json::json!({"concept": "x", "content": "y", "confidence": 0.9}));
    let result = orch.process_event(&event);
    assert!(!result.incorporated);
    assert!(result.reason.contains("self-published"));
}

#[test]
fn process_event_applies_peer_discount() {
    let mut orch = HiveMindOrchestrator::with_default_policy()
        .with_agent_id("me".to_string());
    let event = make_event("FACT_PROMOTED", "peer",
        serde_json::json!({"concept": "bio", "content": "DNA", "confidence": 0.8}));
    let result = orch.process_event(&event);
    assert!(result.incorporated);
    // Discounted confidence should be 0.8 * 0.9 = 0.72
    let facts = orch.query("bio").unwrap();
    assert!(!facts.is_empty());
    assert!((facts[0].confidence - 0.72).abs() < 0.01);
}

// --- drain_events tests ---

#[test]
fn drain_events_returns_pending() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let event = make_event("OTHER_TYPE", "src", serde_json::json!({}));
    orch.process_event(&event); // non-FACT_PROMOTED goes to pending
    let results = orch.drain_events();
    assert_eq!(results.len(), 1);
    // Should be empty after drain
    assert!(orch.drain_raw_events().is_empty());
}

// --- peer management tests ---

#[test]
fn add_peer_increments_count() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    assert_eq!(orch.peer_count(), 0);
    let peer = HiveMindOrchestrator::with_default_policy();
    orch.add_peer(peer);
    assert_eq!(orch.peer_count(), 1);
}

#[test]
fn all_peer_facts_collects() {
    let mut peer = HiveMindOrchestrator::with_default_policy();
    peer.store_fact("rust", "fast", 0.9, "p1").unwrap();

    let mut orch = HiveMindOrchestrator::with_default_policy();
    orch.add_peer(peer);

    let facts = orch.all_peer_facts();
    assert_eq!(facts.len(), 1);
}

// --- close tests ---

#[test]
fn close_marks_orchestrator_closed() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    assert!(!orch.is_closed());
    orch.close().unwrap();
    assert!(orch.is_closed());
}

#[test]
fn close_clears_peers() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    orch.add_peer(HiveMindOrchestrator::with_default_policy());
    orch.close().unwrap();
    assert_eq!(orch.peer_count(), 0);
}

// --- store_fact + query + promote tests ---

#[test]
fn orchestrator_store_fact() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let id = orch.store_fact("rust", "systems language", 0.9, "agent-1").unwrap();
    assert!(!id.is_empty());
}

#[test]
fn orchestrator_query() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    orch.store_fact("rust", "systems language", 0.9, "agent-1").unwrap();
    let facts = orch.query("rust").unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
}

#[test]
fn orchestrator_promote() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let id = orch.store_fact("rust", "systems language", 0.9, "agent-1").unwrap();
    let promoted = orch.promote(&id, "agent-1").unwrap();
    assert!(promoted);
}

#[test]
fn orchestrator_promote_below_threshold() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let id = orch.store_fact("rumor", "unverified", 0.3, "agent-1").unwrap();
    let promoted = orch.promote(&id, "agent-1").unwrap();
    assert!(!promoted);
}

#[test]
fn orchestrator_promote_nonexistent() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let promoted = orch.promote("nonexistent-id", "agent-1").unwrap();
    assert!(!promoted);
}

// --- gossip round tests ---

#[test]
fn gossip_round_no_gossip_protocol() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    let result = orch.run_gossip_round();
    assert!(result.skipped.is_some());
    assert!(result.skipped.unwrap().contains("gossip protocol not configured"));
}

#[test]
fn gossip_round_no_peers() {
    let mut orch = HiveMindOrchestrator::with_default_policy();
    orch.set_gossip(GossipProtocol::new("node-1".into(), GossipConfig::default()));
    let result = orch.run_gossip_round();
    assert!(result.skipped.is_some());
    assert!(result.skipped.unwrap().contains("no peers"));
}

#[test]
fn gossip_round_with_peers() {
    let mut peer = HiveMindOrchestrator::with_default_policy();
    peer.store_fact("bio", "DNA", 0.8, "peer").unwrap();

    let mut orch = HiveMindOrchestrator::with_default_policy();
    orch.store_fact("bio", "RNA", 0.9, "local").unwrap();
    orch.set_gossip(GossipProtocol::new("node-1".into(), GossipConfig::default()));
    orch.add_peer(peer);

    let result = orch.run_gossip_round();
    assert!(result.skipped.is_none());
    assert!(result.peers_contacted > 0);
}
