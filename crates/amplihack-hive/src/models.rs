use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
}

/// An event published to the hive event bus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BusEvent {
    pub topic: String,
    pub payload: serde_json::Value,
    pub source_id: String,
    pub timestamp: DateTime<Utc>,
}

/// Specification for a single agent type within the hive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    pub name: String,
    pub role: String,
    pub replicas: u32,
    pub memory_config: Option<String>,
}

/// Desired-state manifest describing an entire hive deployment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HiveManifest {
    pub agents: Vec<AgentSpec>,
    pub graph_config: serde_json::Value,
    pub event_bus_config: serde_json::Value,
    pub gateway_config: serde_json::Value,
}

/// Snapshot of current hive runtime state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HiveState {
    pub running_agents: Vec<AgentSpec>,
    pub graph_status: String,
    pub bus_status: String,
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
        Self {
            fanout: 3,
            interval_ms: 1000,
            min_confidence: 0.5,
        }
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
    /// Fact IDs that were accepted.
    pub accepted: Vec<String>,
    /// Fact IDs that were rejected.
    pub rejected: Vec<String>,
    /// Fact IDs that had conflicts.
    pub conflicts: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_fact() -> HiveFact {
        HiveFact {
            fact_id: "f-1".into(),
            concept: "rust".into(),
            content: "Rust is a systems language".into(),
            confidence: 0.95,
            source_id: "agent-a".into(),
            tags: vec!["lang".into(), "systems".into()],
            created_at: Utc::now(),
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
    fn bus_event_serde_roundtrip() {
        let event = BusEvent {
            topic: "knowledge.update".into(),
            payload: serde_json::json!({"key": "value"}),
            source_id: "bus-1".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: BusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
    }

    #[test]
    fn agent_spec_serde_roundtrip() {
        let spec = AgentSpec {
            name: "researcher".into(),
            role: "research".into(),
            replicas: 3,
            memory_config: Some("shared".into()),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let decoded: AgentSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, decoded);
    }

    #[test]
    fn hive_manifest_serde_roundtrip() {
        let manifest = HiveManifest {
            agents: vec![AgentSpec {
                name: "worker".into(),
                role: "compute".into(),
                replicas: 2,
                memory_config: None,
            }],
            graph_config: serde_json::json!({}),
            event_bus_config: serde_json::json!({"capacity": 100}),
            gateway_config: serde_json::json!({"port": 8080}),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let decoded: HiveManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest, decoded);
    }

    #[test]
    fn hive_state_serde_roundtrip() {
        let state = HiveState {
            running_agents: vec![],
            graph_status: "ready".into(),
            bus_status: "connected".into(),
        };
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
        let msg = GossipMessage {
            facts: vec![sample_fact()],
            source_id: "node-1".into(),
            round: 42,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: GossipMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn merge_result_serde_roundtrip() {
        let result = MergeResult {
            accepted: vec!["f-1".into()],
            rejected: vec!["f-2".into()],
            conflicts: vec!["f-3".into()],
        };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: MergeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, decoded);
    }
}
