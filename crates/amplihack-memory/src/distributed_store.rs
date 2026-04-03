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

/// Exported node representation: `(table, node_id, properties)` — matches [`NodeTriple`].
type ExportedNodes = Vec<(String, String, Props)>;
/// Exported edge representation: `(rel_type, from_id, to_id, properties)` — matches [`EdgeQuad`].
type ExportedEdges = Vec<(String, String, String, Props)>;

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
    next_id: u64,
}

impl DistributedGraphStore {
    pub fn new(config: DistributedConfig) -> Self {
        Self {
            ring: HashRing::default_ring(),
            shards: HashMap::new(),
            config,
            next_id: 0,
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

        // Pre-generate a stable monotonic ID so all replicas store the same node ID.
        let mut props = properties.clone();
        if !props.contains_key("id") {
            self.next_id += 1;
            let id = format!("dist-{}", self.next_id);
            props.insert("id".into(), serde_json::json!(id));
        }

        let mut node_id = None;
        for owner in &owners {
            if let Some(shard) = self.shards.get_mut(owner) {
                let id = shard.store.create_node(table, &props)?;
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
        for shard in self.shards.values() {
            if shard.bloom.might_contain(node_id)
                && let Some(props) = shard.store.get_node(table, node_id)?
            {
                return Ok(Some(props));
            }
        }
        // Fallback: full scan
        for shard in self.shards.values() {
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

            let (n, e) = self.sync_pair(a_id, b_id)?;
            total_nodes += n;
            total_edges += e;

            if n > 0 || e > 0 {
                debug!(from = %a_id, to = %b_id, nodes = n, edges = e, "Gossip sync");
            }
        }

        info!(
            nodes = total_nodes,
            edges = total_edges,
            "Gossip round complete"
        );
        Ok((total_nodes, total_edges))
    }

    /// Sync missing data from shard `from` into shard `to`.
    fn sync_pair(&mut self, from: &str, to: &str) -> anyhow::Result<(usize, usize)> {
        let from_ids = self.get_shard_node_ids(from);
        let missing = self.find_missing_nodes(to, &from_ids);
        if missing.is_empty() {
            return Ok((0, 0));
        }

        let (nodes_export, edges_export) = self.export_from_shard(from, &missing);
        self.import_into_shard(to, &nodes_export, &edges_export)
    }

    fn get_shard_node_ids(&self, agent_id: &str) -> Vec<String> {
        self.shards
            .get(agent_id)
            .map(|s| {
                s.store
                    .get_all_node_ids(None)
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn find_missing_nodes(&self, target: &str, candidate_ids: &[String]) -> Vec<String> {
        match self.shards.get(target) {
            Some(shard) => {
                let refs: Vec<&str> = candidate_ids.iter().map(|s| s.as_str()).collect();
                shard
                    .bloom
                    .missing_from(&refs)
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            }
            None => Vec::new(),
        }
    }

    fn export_from_shard(
        &self,
        agent_id: &str,
        node_ids: &[String],
    ) -> (ExportedNodes, ExportedEdges) {
        let nodes = self
            .shards
            .get(agent_id)
            .map(|s| s.store.export_nodes(Some(node_ids)).unwrap_or_default())
            .unwrap_or_default();
        let edges = self
            .shards
            .get(agent_id)
            .map(|s| s.store.export_edges(Some(node_ids)).unwrap_or_default())
            .unwrap_or_default();
        (nodes, edges)
    }

    fn import_into_shard(
        &mut self,
        agent_id: &str,
        nodes: &[(String, String, Props)],
        edges: &[(String, String, String, Props)],
    ) -> anyhow::Result<(usize, usize)> {
        if let Some(shard) = self.shards.get_mut(agent_id) {
            let n = shard.store.import_nodes(nodes)?;
            let e = shard.store.import_edges(edges)?;
            for (_, id, _) in nodes {
                shard.bloom.add(id);
            }
            Ok((n, e))
        } else {
            Ok((0, 0))
        }
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
#[path = "tests/distributed_store_tests.rs"]
mod tests;
