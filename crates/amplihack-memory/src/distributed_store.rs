//! DHT-sharded distributed graph store.
//!
//! Matches Python `amplihack/memory/distributed_store.py`:
//! - Consistent hash ring for content → agent routing
//! - Replication factor (default 3)
//! - Query fanout across shards
//! - Bloom filter gossip protocol
//! - Shard rebuild/recovery

use crate::bloom::BloomFilter;
use crate::graph_store::{GraphStore, Props};
use crate::hash_ring::HashRing;
use crate::memory_store::InMemoryGraphStore;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

/// Configuration for the distributed store.
pub struct DistributedConfig {
    pub replication_factor: usize,
    pub query_fanout: usize,
}

impl Default for DistributedConfig {
    fn default() -> Self {
        Self {
            replication_factor: 3,
            query_fanout: 5,
        }
    }
}

struct AgentShard {
    store: InMemoryGraphStore,
    bloom: BloomFilter,
}

impl AgentShard {
    fn new() -> Self {
        Self {
            store: InMemoryGraphStore::new(),
            bloom: BloomFilter::new(10_000, 0.01),
        }
    }
}

/// Distributed graph store with DHT sharding and gossip protocol.
pub struct DistributedGraphStore {
    ring: HashRing,
    shards: HashMap<String, AgentShard>,
    config: DistributedConfig,
}

impl DistributedGraphStore {
    pub fn new(config: DistributedConfig) -> Self {
        Self {
            ring: HashRing::default_ring(),
            shards: HashMap::new(),
            config,
        }
    }

    /// Add an agent to the distributed store.
    pub fn add_agent(&mut self, agent_id: &str) {
        self.ring.add_agent(agent_id);
        self.shards
            .entry(agent_id.to_string())
            .or_insert_with(AgentShard::new);
        info!(agent_id, "Added agent to distributed store");
    }

    /// Remove an agent from the distributed store.
    pub fn remove_agent(&mut self, agent_id: &str) {
        self.ring.remove_agent(agent_id);
        self.shards.remove(agent_id);
    }

    /// Create a node, replicating to `replication_factor` agents.
    pub fn create_node(&mut self, table: &str, properties: &Props) -> anyhow::Result<String> {
        let content_key = properties
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        let owners = self
            .ring
            .get_agents(content_key, self.config.replication_factor)
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();

        let mut node_id = None;
        for owner in &owners {
            if let Some(shard) = self.shards.get_mut(owner) {
                let id = shard.store.create_node(table, properties)?;
                shard.bloom.add(&id);
                if node_id.is_none() {
                    node_id = Some(id);
                }
            }
        }
        node_id.ok_or_else(|| anyhow::anyhow!("No shards available"))
    }

    /// Get a node by ID, checking bloom filters first.
    pub fn get_node(&self, table: &str, node_id: &str) -> anyhow::Result<Option<Props>> {
        // Check bloom filters for likely shard
        for (_, shard) in &self.shards {
            if shard.bloom.might_contain(node_id) {
                if let Some(props) = shard.store.get_node(table, node_id)? {
                    return Ok(Some(props));
                }
            }
        }
        // Fallback: full scan
        for (_, shard) in &self.shards {
            if let Some(props) = shard.store.get_node(table, node_id)? {
                return Ok(Some(props));
            }
        }
        Ok(None)
    }

    /// Search across shards with fanout.
    pub fn search_nodes(
        &self,
        table: &str,
        text: &str,
        fields: Option<&[&str]>,
        limit: usize,
    ) -> anyhow::Result<Vec<Props>> {
        let mut results = Vec::new();
        let mut seen_ids = HashSet::new();
        let fanout = self.config.query_fanout.min(self.shards.len());

        for (i, (_, shard)) in self.shards.iter().enumerate() {
            if i >= fanout {
                break;
            }
            for props in shard.store.search_nodes(table, text, fields, limit)? {
                let id = props
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if seen_ids.insert(id) {
                    results.push(props);
                }
            }
        }
        results.truncate(limit);
        Ok(results)
    }

    /// Run one gossip round between consecutive shard pairs.
    /// Returns (nodes_synced, edges_synced).
    pub fn run_gossip_round(&mut self) -> anyhow::Result<(usize, usize)> {
        let agent_ids: Vec<String> = self.ring.agents().to_vec();
        if agent_ids.len() < 2 {
            return Ok((0, 0));
        }

        let mut total_nodes = 0;
        let mut total_edges = 0;

        for i in 0..agent_ids.len() {
            let j = (i + 1) % agent_ids.len();
            let a_id = &agent_ids[i];
            let b_id = &agent_ids[j];

            // Get node IDs from shard A
            let a_ids: Vec<String> = self
                .shards
                .get(a_id)
                .map(|s| {
                    s.store
                        .get_all_node_ids(None)
                        .unwrap_or_default()
                        .into_iter()
                        .collect()
                })
                .unwrap_or_default();

            // Find which of A's nodes B is missing (bloom filter check)
            let missing: Vec<String> = if let Some(shard_b) = self.shards.get(b_id) {
                let refs: Vec<&str> = a_ids.iter().map(|s| s.as_str()).collect();
                shard_b.bloom.missing_from(&refs).iter().map(|s| s.to_string()).collect()
            } else {
                continue;
            };

            if missing.is_empty() {
                continue;
            }

            // Export missing nodes from A
            let nodes_export = self
                .shards
                .get(a_id)
                .map(|s| s.store.export_nodes(Some(&missing)).unwrap_or_default())
                .unwrap_or_default();
            let edges_export = self
                .shards
                .get(a_id)
                .map(|s| s.store.export_edges(Some(&missing)).unwrap_or_default())
                .unwrap_or_default();

            // Import into B
            if let Some(shard_b) = self.shards.get_mut(b_id) {
                let n = shard_b.store.import_nodes(&nodes_export)?;
                let e = shard_b.store.import_edges(&edges_export)?;
                for (_, id, _) in &nodes_export {
                    shard_b.bloom.add(id);
                }
                total_nodes += n;
                total_edges += e;
            }

            debug!(from = a_id, to = b_id, nodes = total_nodes, "Gossip sync");
        }

        info!(nodes = total_nodes, edges = total_edges, "Gossip round complete");
        Ok((total_nodes, total_edges))
    }

    /// Rebuild a shard by pulling data from peers.
    pub fn rebuild_shard(&mut self, agent_id: &str) -> anyhow::Result<usize> {
        // Collect all nodes from other shards
        let mut all_nodes = Vec::new();
        let mut all_edges = Vec::new();
        for (id, shard) in &self.shards {
            if id == agent_id {
                continue;
            }
            all_nodes.extend(shard.store.export_nodes(None)?);
            all_edges.extend(shard.store.export_edges(None)?);
        }

        // Import into target shard
        let shard = self
            .shards
            .get_mut(agent_id)
            .ok_or_else(|| anyhow::anyhow!("Agent not found: {agent_id}"))?;
        let n = shard.store.import_nodes(&all_nodes)?;
        let _e = shard.store.import_edges(&all_edges)?;
        for (_, id, _) in &all_nodes {
            shard.bloom.add(id);
        }
        info!(agent_id, recovered = n, "Shard rebuilt");
        Ok(n)
    }

    /// Number of agents in the store.
    pub fn agent_count(&self) -> usize {
        self.shards.len()
    }

    /// Get statistics for a specific agent's shard.
    pub fn shard_stats(&self, agent_id: &str) -> Option<ShardStats> {
        self.shards.get(agent_id).map(|shard| {
            let node_count = shard.store.get_all_node_ids(None).unwrap_or_default().len();
            ShardStats {
                agent_id: agent_id.to_string(),
                node_count,
                bloom_count: shard.bloom.count(),
                bloom_size_bytes: shard.bloom.size_bytes(),
            }
        })
    }
}

/// Statistics for a single shard.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShardStats {
    pub agent_id: String,
    pub node_count: usize,
    pub bloom_count: usize,
    pub bloom_size_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_props(content: &str) -> Props {
        let mut p = Props::new();
        p.insert("content".into(), json!(content));
        p
    }

    #[test]
    fn add_agents_and_create_node() {
        let mut store = DistributedGraphStore::new(DistributedConfig::default());
        store.add_agent("a1");
        store.add_agent("a2");
        let id = store.create_node("test", &make_props("hello world")).unwrap();
        let node = store.get_node("test", &id).unwrap();
        assert!(node.is_some());
    }

    #[test]
    fn search_across_shards() {
        let mut store = DistributedGraphStore::new(DistributedConfig {
            replication_factor: 1,
            query_fanout: 10,
        });
        store.add_agent("a1");
        store.add_agent("a2");
        store.create_node("t", &make_props("sky is blue")).unwrap();
        store.create_node("t", &make_props("grass is green")).unwrap();
        let results = store.search_nodes("t", "sky", None, 10).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn gossip_syncs_between_shards() {
        let mut store = DistributedGraphStore::new(DistributedConfig {
            replication_factor: 1,
            query_fanout: 10,
        });
        store.add_agent("a1");
        store.add_agent("a2");

        // Create nodes only on a1's shard
        store.shards.get_mut("a1").unwrap().store
            .create_node("t", &make_props("exclusive data")).unwrap();

        let (nodes, _) = store.run_gossip_round().unwrap();
        assert!(nodes > 0, "gossip should sync at least one node");
    }

    #[test]
    fn rebuild_shard_recovers_data() {
        let mut store = DistributedGraphStore::new(DistributedConfig {
            replication_factor: 1,
            query_fanout: 10,
        });
        store.add_agent("a1");
        store.add_agent("a2");

        // Add data to a1
        store.shards.get_mut("a1").unwrap().store
            .create_node("t", &make_props("data to recover")).unwrap();

        let recovered = store.rebuild_shard("a2").unwrap();
        assert!(recovered > 0);
    }

    #[test]
    fn shard_stats() {
        let mut store = DistributedGraphStore::new(DistributedConfig::default());
        store.add_agent("a1");
        store.create_node("t", &make_props("test data")).unwrap();
        let stats = store.shard_stats("a1").unwrap();
        assert!(stats.bloom_size_bytes > 0);
    }

    #[test]
    fn remove_agent() {
        let mut store = DistributedGraphStore::new(DistributedConfig::default());
        store.add_agent("a1");
        store.add_agent("a2");
        assert_eq!(store.agent_count(), 2);
        store.remove_agent("a1");
        assert_eq!(store.agent_count(), 1);
    }

    #[test]
    fn no_shards_returns_error() {
        let mut store = DistributedGraphStore::new(DistributedConfig::default());
        let result = store.create_node("t", &make_props("orphan"));
        assert!(result.is_err());
    }
}
