use amplihack_hive::{GossipConfig, GossipMessage, GossipProtocol, HiveFact, MergeResult};
use chrono::Utc;

fn default_config() -> GossipConfig {
    GossipConfig {
        fanout: 3,
        interval_ms: 1000,
        min_confidence: 0.5,
    }
}

fn make_fact(concept: &str, confidence: f64) -> HiveFact {
    HiveFact {
        fact_id: format!("fact-{concept}"),
        concept: concept.to_string(),
        content: format!("{concept} content"),
        confidence,
        source_id: "test-node".to_string(),
        tags: vec![],
        created_at: Utc::now(),
        status: "promoted".to_string(),
        metadata: std::collections::HashMap::new(),
    }
}

// --- accessor tests (REAL implementations, should pass) ---

#[test]
fn new_protocol_has_zero_round() {
    let proto = GossipProtocol::new("node-1".to_string(), default_config());
    assert_eq!(proto.current_round(), 0);
}

#[test]
fn new_protocol_stores_node_id() {
    let proto = GossipProtocol::new("node-alpha".to_string(), default_config());
    assert_eq!(proto.node_id(), "node-alpha");
}

#[test]
fn new_protocol_stores_config() {
    let config = GossipConfig {
        fanout: 5,
        interval_ms: 2000,
        min_confidence: 0.7,
    };
    let proto = GossipProtocol::new("node-1".to_string(), config);
    let cfg = proto.config();
    assert_eq!(cfg.fanout, 5);
    assert_eq!(cfg.interval_ms, 2000);
    assert!((cfg.min_confidence - 0.7).abs() < f64::EPSILON);
}

#[test]
fn config_default_values() {
    let config = default_config();
    assert_eq!(config.fanout, 3);
    assert_eq!(config.interval_ms, 1000);
    assert!((config.min_confidence - 0.5).abs() < f64::EPSILON);
}

#[test]
fn gossip_config_serde_roundtrip() {
    let config = GossipConfig {
        fanout: 4,
        interval_ms: 500,
        min_confidence: 0.6,
    };
    let json = serde_json::to_string(&config).unwrap();
    let deserialized: GossipConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.fanout, 4);
    assert_eq!(deserialized.interval_ms, 500);
    assert!((deserialized.min_confidence - 0.6).abs() < f64::EPSILON);
}

#[test]
fn gossip_message_serde_roundtrip() {
    let msg = GossipMessage {
        facts: vec![make_fact("test", 0.8)],
        source_id: "node-1".to_string(),
        round: 3,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let deserialized: GossipMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.source_id, "node-1");
    assert_eq!(deserialized.round, 3);
    assert_eq!(deserialized.facts.len(), 1);
}

#[test]
fn merge_result_serde_roundtrip() {
    let result = MergeResult {
        accepted: vec!["a".to_string()],
        rejected: vec!["b".to_string()],
        conflicts: vec!["c".to_string()],
    };
    let json = serde_json::to_string(&result).unwrap();
    let deserialized: MergeResult = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.accepted, vec!["a"]);
    assert_eq!(deserialized.rejected, vec!["b"]);
    assert_eq!(deserialized.conflicts, vec!["c"]);
}

// --- behavioral tests ---

#[test]
fn run_gossip_round_merges_facts() {
    let mut proto = GossipProtocol::new("node-1".to_string(), default_config());
    let local = vec![make_fact("local", 0.9)];
    let peer = vec![make_fact("peer", 0.8)];
    let result = proto.run_gossip_round(&local, &peer).unwrap();
    assert_eq!(result.accepted, vec!["fact-peer"]);
    assert!(result.rejected.is_empty());
    assert!(result.conflicts.is_empty());
    assert_eq!(proto.current_round(), 1);
}

#[test]
fn run_gossip_round_detects_conflicts() {
    let mut proto = GossipProtocol::new("node-1".to_string(), default_config());
    let local = vec![make_fact("shared", 0.9)];
    let peer = vec![make_fact("shared", 0.8)];
    let result = proto.run_gossip_round(&local, &peer).unwrap();
    assert!(result.accepted.is_empty());
    assert_eq!(result.conflicts, vec!["fact-shared"]);
}

#[test]
fn run_gossip_round_rejects_low_confidence() {
    let mut proto = GossipProtocol::new("node-1".to_string(), default_config());
    let local = vec![];
    let peer = vec![make_fact("weak", 0.3)];
    let result = proto.run_gossip_round(&local, &peer).unwrap();
    assert!(result.accepted.is_empty());
    assert_eq!(result.rejected, vec!["fact-weak"]);
}

#[test]
fn prepare_message_creates_gossip_message() {
    let proto = GossipProtocol::new("node-1".to_string(), default_config());
    let facts = vec![make_fact("test", 0.7)];
    let msg = proto.prepare_message(facts);
    assert_eq!(msg.source_id, "node-1");
    assert_eq!(msg.round, 0);
    assert_eq!(msg.facts.len(), 1);
    assert_eq!(msg.facts[0].fact_id, "fact-test");
}

#[test]
fn merge_incoming_processes_message() {
    let mut proto = GossipProtocol::new("node-1".to_string(), default_config());
    let msg = GossipMessage {
        facts: vec![make_fact("incoming", 0.8)],
        source_id: "node-2".to_string(),
        round: 1,
    };
    let result = proto.merge_incoming(msg).unwrap();
    assert_eq!(result.accepted, vec!["fact-incoming"]);
    assert!(result.rejected.is_empty());
    assert_eq!(proto.current_round(), 1);
}

#[test]
fn select_peers_limits_by_fanout() {
    let proto = GossipProtocol::new("node-1".to_string(), default_config());
    let all_peers: Vec<String> = (0..10).map(|i| format!("peer-{i}")).collect();
    let selected = proto.select_peers(&all_peers);
    assert_eq!(selected.len(), 3); // fanout = 3
}

#[test]
fn select_peers_empty_list() {
    let proto = GossipProtocol::new("node-1".to_string(), default_config());
    let selected = proto.select_peers(&[]);
    assert!(selected.is_empty());
}

#[test]
fn select_peers_fewer_than_fanout() {
    let proto = GossipProtocol::new("node-1".to_string(), default_config());
    let all_peers = vec!["peer-0".to_string(), "peer-1".to_string()];
    let selected = proto.select_peers(&all_peers);
    assert_eq!(selected.len(), 2); // only 2 available, fanout is 3
}
