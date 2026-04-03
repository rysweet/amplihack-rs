use std::collections::HashMap;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

// ── Python-ported constants ────────────────────────────────────────
pub const DEFAULT_QUALITY_THRESHOLD: f64 = 0.3;
pub const DEFAULT_CONFIDENCE_GATE: f64 = 0.3;
pub const DEFAULT_BROADCAST_THRESHOLD: f64 = 0.9;
pub const GOSSIP_MIN_CONFIDENCE: f64 = 0.3;
pub const PEER_CONFIDENCE_DISCOUNT: f64 = 0.9;
pub const DEFAULT_CONTRADICTION_OVERLAP: f64 = 0.4;
pub const MAX_TRUST_SCORE: f64 = 2.0;
pub const DEFAULT_TRUST_SCORE: f64 = 1.0;
pub const FACT_ID_HEX_LENGTH: usize = 12;

fn default_fact_status() -> String {
    "promoted".to_string()
}

/// A single fact stored in the hive knowledge graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HiveFact {
    pub fact_id: String,
    pub concept: String,
    pub content: String,
    pub confidence: f64,
    pub source_id: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default = "default_fact_status")]
    pub status: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// An event published to the hive event bus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BusEvent {
    pub event_id: String,
    /// Kept for backward compat; also accessible as `event_type`.
    #[serde(alias = "event_type")]
    pub topic: String,
    pub payload: serde_json::Value,
    /// Kept for backward compat; also accessible as `source_agent`.
    #[serde(alias = "source_agent")]
    pub source_id: String,
    pub timestamp: DateTime<Utc>,
}

impl BusEvent {
    /// Create a new BusEvent with auto-generated ID and timestamp.
    pub fn new(topic: &str, payload: serde_json::Value, source_id: &str) -> Self {
        Self {
            event_id: uuid::Uuid::new_v4().to_string(),
            topic: topic.to_string(),
            payload,
            source_id: source_id.to_string(),
            timestamp: Utc::now(),
        }
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
    pub fn from_json(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value.clone())
    }
}

/// Factory for creating a [`BusEvent`] with a generated id and current timestamp.
pub fn make_event(topic: impl Into<String>, source_id: impl Into<String>, payload: serde_json::Value) -> BusEvent {
    BusEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        topic: topic.into(),
        payload,
        source_id: source_id.into(),
        timestamp: Utc::now(),
    }
}

/// Specification for a single agent type within the hive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    pub name: String,
    pub role: String,
    pub replicas: u32,
    pub memory_config: Option<String>,
    #[serde(default)]
    pub domain: String,
}

/// An agent registered in the hive knowledge graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HiveAgent {
    pub agent_id: String,
    pub domain: String,
    pub trust: f64,
    pub fact_count: u64,
    pub status: String,
}

impl HiveAgent {
    pub fn new(agent_id: impl Into<String>, domain: impl Into<String>) -> Self {
        Self { agent_id: agent_id.into(), domain: domain.into(), trust: DEFAULT_TRUST_SCORE, fact_count: 0, status: "active".into() }
    }
}

/// A directed edge between two nodes in the knowledge graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HiveEdge {
    pub source_id: String,
    pub target_id: String,
    pub edge_type: String,
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

/// Summary statistics for a [`crate::graph::HiveGraph`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphStats {
    pub fact_count: usize,
    pub agent_count: usize,
    pub edge_count: usize,
    pub retracted_count: usize,
    pub active_agent_count: usize,
}

// ── Typed config structs ───────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphStoreConfig {
    #[serde(default = "default_graph_backend")]
    pub backend: String,
}
fn default_graph_backend() -> String { "memory".into() }
impl Default for GraphStoreConfig { fn default() -> Self { Self { backend: default_graph_backend() } } }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventBusConfig {
    #[serde(default = "default_bus_type")]
    pub bus_type: String,
}
fn default_bus_type() -> String { "local".into() }
impl Default for EventBusConfig { fn default() -> Self { Self { bus_type: default_bus_type() } } }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GatewayConfig {
    #[serde(default)]
    pub trust_threshold: f64,
    #[serde(default = "default_contradiction_overlap")]
    pub contradiction_overlap: f64,
}
fn default_contradiction_overlap() -> f64 { DEFAULT_CONTRADICTION_OVERLAP }
impl Default for GatewayConfig {
    fn default() -> Self { Self { trust_threshold: 0.0, contradiction_overlap: DEFAULT_CONTRADICTION_OVERLAP } }
}

/// Desired-state manifest describing an entire hive deployment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HiveManifest {
    pub agents: Vec<AgentSpec>,
    // Legacy JSON fields kept for backward compat
    #[serde(default)]
    pub graph_config: serde_json::Value,
    #[serde(default)]
    pub event_bus_config: serde_json::Value,
    #[serde(default)]
    pub gateway_config: serde_json::Value,
    // Typed config fields
    #[serde(default)]
    pub graph_store: GraphStoreConfig,
    #[serde(default)]
    pub event_bus: EventBusConfig,
    #[serde(default)]
    pub gateway: GatewayConfig,
}

impl HiveManifest {
    /// Parse from a [`serde_json::Value`], substituting `${ENV_VAR}` patterns.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        let raw = serde_json::to_string(&value)?;
        let re = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("valid regex");
        let substituted = re.replace_all(&raw, |caps: &regex::Captures| {
            std::env::var(&caps[1]).unwrap_or_default()
        });
        serde_json::from_str(&substituted)
    }
}

/// Snapshot of current hive runtime state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HiveState {
    pub running_agents: Vec<AgentSpec>,
    pub graph_status: String,
    pub bus_status: String,
    #[serde(default)]
    pub agents: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub hive_store_connected: bool,
    #[serde(default)]
    pub event_bus_connected: bool,
}

/// Configuration for the gossip protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GossipConfig {
    pub fanout: usize,
    pub interval_ms: u64,
    pub min_confidence: f64,
}

impl Default for GossipConfig {
    fn default() -> Self {
        Self { fanout: 3, interval_ms: 1000, min_confidence: 0.5 }
    }
}

/// A gossip message exchanged between nodes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GossipMessage {
    pub facts: Vec<HiveFact>,
    pub source_id: String,
    pub round: u64,
}

/// Outcome of a gossip merge operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MergeResult {
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
    pub conflicts: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fact() -> HiveFact {
        HiveFact {
            fact_id: "f-1".into(), concept: "rust".into(),
            content: "Rust is a systems language".into(), confidence: 0.95,
            source_id: "agent-a".into(), tags: vec!["lang".into(), "systems".into()],
            created_at: Utc::now(), status: "promoted".into(), metadata: HashMap::new(),
        }
    }

    #[test]
    fn hive_fact_serde_roundtrip() {
        let fact = sample_fact();
        let json = serde_json::to_string(&fact).unwrap();
        let decoded: HiveFact = serde_json::from_str(&json).unwrap();
        assert_eq!(fact, decoded);
    }

    #[test]
    fn hive_fact_default_status_on_deser() {
        let json = r#"{"fact_id":"x","concept":"c","content":"t","confidence":0.5,
            "source_id":"a","tags":[],"created_at":"2024-01-01T00:00:00Z"}"#;
        let f: HiveFact = serde_json::from_str(json).unwrap();
        assert_eq!(f.status, "promoted");
        assert!(f.metadata.is_empty());
    }

    #[test]
    fn bus_event_serde_roundtrip() {
        let event = make_event("knowledge.update", "bus-1", serde_json::json!({"key": "value"}));
        let json = serde_json::to_string(&event).unwrap();
        let decoded: BusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event.topic, decoded.topic);
    }

    #[test]
    fn bus_event_json_methods() {
        let event = make_event("test", "src", serde_json::json!({}));
        let v = event.to_json();
        let back = BusEvent::from_json(&v).unwrap();
        assert_eq!(back.topic, "test");
    }

    #[test]
    fn agent_spec_serde_roundtrip() {
        let spec = AgentSpec { name: "researcher".into(), role: "research".into(),
            replicas: 3, memory_config: Some("shared".into()), domain: String::new() };
        let json = serde_json::to_string(&spec).unwrap();
        let decoded: AgentSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, decoded);
    }

    #[test]
    fn agent_spec_domain_defaults() {
        let json = r#"{"name":"w","role":"r","replicas":1,"memory_config":null}"#;
        let s: AgentSpec = serde_json::from_str(json).unwrap();
        assert_eq!(s.domain, "");
    }

    #[test]
    fn hive_manifest_serde_roundtrip() {
        let manifest = HiveManifest {
            agents: vec![AgentSpec { name: "worker".into(), role: "compute".into(),
                replicas: 2, memory_config: None, domain: String::new() }],
            graph_config: serde_json::json!({}),
            event_bus_config: serde_json::json!({"capacity": 100}),
            gateway_config: serde_json::json!({"port": 8080}),
            graph_store: GraphStoreConfig::default(),
            event_bus: EventBusConfig::default(),
            gateway: GatewayConfig::default(),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let decoded: HiveManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest, decoded);
    }

    #[test]
    fn hive_manifest_from_value_env_sub() {
        // SAFETY: test-only env manipulation; tests in this module run serially.
        unsafe { std::env::set_var("TEST_HIVE_PORT", "9090"); }
        let v = serde_json::json!({
            "agents": [], "gateway": {"trust_threshold": 0.0,
                "contradiction_overlap": 0.4}
        });
        let m = HiveManifest::from_value(v).unwrap();
        assert!(m.agents.is_empty());
        unsafe { std::env::remove_var("TEST_HIVE_PORT"); }
    }

    #[test]
    fn hive_state_serde_roundtrip() {
        let state = HiveState { running_agents: vec![], graph_status: "ready".into(),
            bus_status: "connected".into(), agents: HashMap::new(),
            hive_store_connected: false, event_bus_connected: false };
        let json = serde_json::to_string(&state).unwrap();
        let decoded: HiveState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, decoded);
    }

    #[test]
    fn gossip_config_default() {
        let config = GossipConfig::default();
        assert_eq!(config.fanout, 3);
        assert_eq!(config.interval_ms, 1000);
        assert!((config.min_confidence - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn gossip_config_serde_roundtrip() {
        let config = GossipConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let decoded: GossipConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, decoded);
    }

    #[test]
    fn gossip_message_serde_roundtrip() {
        let msg = GossipMessage { facts: vec![sample_fact()], source_id: "node-1".into(), round: 42 };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: GossipMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn merge_result_serde_roundtrip() {
        let result = MergeResult { accepted: vec!["f-1".into()], rejected: vec!["f-2".into()],
            conflicts: vec!["f-3".into()] };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: MergeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, decoded);
    }

    #[test]
    fn constants_match_python() {
        assert!((DEFAULT_QUALITY_THRESHOLD - 0.3).abs() < f64::EPSILON);
        assert!((DEFAULT_CONFIDENCE_GATE - 0.3).abs() < f64::EPSILON);
        assert!((DEFAULT_BROADCAST_THRESHOLD - 0.9).abs() < f64::EPSILON);
        assert!((GOSSIP_MIN_CONFIDENCE - 0.3).abs() < f64::EPSILON);
        assert!((PEER_CONFIDENCE_DISCOUNT - 0.9).abs() < f64::EPSILON);
        assert!((DEFAULT_CONTRADICTION_OVERLAP - 0.4).abs() < f64::EPSILON);
        assert!((MAX_TRUST_SCORE - 2.0).abs() < f64::EPSILON);
        assert!((DEFAULT_TRUST_SCORE - 1.0).abs() < f64::EPSILON);
        assert_eq!(FACT_ID_HEX_LENGTH, 12);
    }
}
