//! Memory system configuration.
//!
//! Matches Python `amplihack/memory/config.py`:
//! - Resolution order: explicit → env vars → defaults
//! - Env var prefix: AMPLIHACK_MEMORY_*
//! - Topology and backend selection

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Memory topology mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Topology {
    /// Local-only agent, no distribution.
    #[default]
    Single,
    /// Multi-agent hive with DHT sharding.
    Distributed,
}

/// Memory backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Backend {
    /// Cognitive adapter with graph DB.
    #[default]
    Cognitive,
    /// Hierarchical memory adapter.
    Hierarchical,
    /// In-memory store (testing/lightweight).
    InMemory,
}

/// Transport mode for distributed topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Transport {
    /// Local in-process transport.
    #[default]
    Local,
    /// Redis pub/sub transport.
    Redis,
    /// Azure Service Bus transport.
    AzureServiceBus,
}

/// Complete memory configuration.
///
/// Resolution order: explicit field → env var → default value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub topology: Topology,
    pub backend: Backend,
    pub transport: Transport,
    pub storage_path: Option<PathBuf>,
    pub replication_factor: usize,
    pub query_fanout: usize,
    pub gossip_enabled: bool,
    pub gossip_rounds: usize,
    pub token_budget_default: usize,
    pub quality_review_enabled: bool,
    pub quality_threshold: f64,
    pub duplicate_detection: bool,
    pub trivial_content_filter: bool,
    pub min_content_length: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            topology: Topology::Single,
            backend: Backend::Cognitive,
            transport: Transport::Local,
            storage_path: None,
            replication_factor: 3,
            query_fanout: 5,
            gossip_enabled: true,
            gossip_rounds: 3,
            token_budget_default: 4000,
            quality_review_enabled: true,
            quality_threshold: 5.0,
            duplicate_detection: true,
            trivial_content_filter: true,
            min_content_length: 10,
        }
    }
}

impl MemoryConfig {
    /// Resolve configuration from environment variables, falling back to defaults.
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(v) = std::env::var("AMPLIHACK_MEMORY_TOPOLOGY") {
            config.topology = match v.to_lowercase().as_str() {
                "distributed" => Topology::Distributed,
                _ => Topology::Single,
            };
        }
        if let Ok(v) = std::env::var("AMPLIHACK_MEMORY_BACKEND") {
            config.backend = match v.to_lowercase().as_str() {
                "hierarchical" => Backend::Hierarchical,
                "inmemory" | "in_memory" => Backend::InMemory,
                _ => Backend::Cognitive,
            };
        }
        if let Ok(v) = std::env::var("AMPLIHACK_MEMORY_TRANSPORT") {
            config.transport = match v.to_lowercase().as_str() {
                "redis" => Transport::Redis,
                "azure_service_bus" | "azureservicebus" => Transport::AzureServiceBus,
                _ => Transport::Local,
            };
        }
        if let Ok(v) = std::env::var("AMPLIHACK_MEMORY_STORAGE_PATH") {
            config.storage_path = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("AMPLIHACK_MEMORY_REPLICATION_FACTOR")
            && let Ok(n) = v.parse()
        {
            config.replication_factor = n;
        }
        if let Ok(v) = std::env::var("AMPLIHACK_MEMORY_QUERY_FANOUT")
            && let Ok(n) = v.parse()
        {
            config.query_fanout = n;
        }
        if let Ok(v) = std::env::var("AMPLIHACK_MEMORY_GOSSIP_ENABLED") {
            config.gossip_enabled = v != "0" && v.to_lowercase() != "false";
        }
        if let Ok(v) = std::env::var("AMPLIHACK_MEMORY_TOKEN_BUDGET")
            && let Ok(n) = v.parse()
        {
            config.token_budget_default = n;
        }
        config
    }

    /// Create a config for testing (in-memory, no quality review).
    pub fn for_testing() -> Self {
        Self {
            backend: Backend::InMemory,
            quality_review_enabled: false,
            gossip_enabled: false,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_python() {
        let cfg = MemoryConfig::default();
        assert_eq!(cfg.topology, Topology::Single);
        assert_eq!(cfg.backend, Backend::Cognitive);
        assert_eq!(cfg.transport, Transport::Local);
        assert_eq!(cfg.replication_factor, 3);
        assert_eq!(cfg.query_fanout, 5);
        assert!(cfg.gossip_enabled);
        assert_eq!(cfg.gossip_rounds, 3);
        assert_eq!(cfg.token_budget_default, 4000);
    }

    #[test]
    fn testing_config_disables_quality() {
        let cfg = MemoryConfig::for_testing();
        assert_eq!(cfg.backend, Backend::InMemory);
        assert!(!cfg.quality_review_enabled);
        assert!(!cfg.gossip_enabled);
    }

    #[test]
    fn config_serializes() {
        let cfg = MemoryConfig::default();
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["topology"], "single");
        assert_eq!(json["backend"], "cognitive");
        assert_eq!(json["replication_factor"], 3);
    }

    #[test]
    fn topology_variants() {
        let single: Topology = serde_json::from_str("\"single\"").unwrap();
        let dist: Topology = serde_json::from_str("\"distributed\"").unwrap();
        assert_eq!(single, Topology::Single);
        assert_eq!(dist, Topology::Distributed);
    }

    #[test]
    fn backend_variants() {
        let c: Backend = serde_json::from_str("\"cognitive\"").unwrap();
        let h: Backend = serde_json::from_str("\"hierarchical\"").unwrap();
        let m: Backend = serde_json::from_str("\"in_memory\"").unwrap();
        assert_eq!(c, Backend::Cognitive);
        assert_eq!(h, Backend::Hierarchical);
        assert_eq!(m, Backend::InMemory);
    }
}
