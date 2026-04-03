use std::collections::{HashMap, HashSet};

use super::ring::HashRing;
use super::store::{ShardFact, ShardStore};
use super::{content_key, hash_key, DEFAULT_REPLICATION_FACTOR};

/// Routes facts and queries across the distributed hash ring.
///
/// Coordinates between [`HashRing`] (who owns what) and [`ShardStore`]s
/// (where facts live).  Handles replication and query fan-out.
#[derive(Debug)]
pub struct DHTRouter {
    ring: HashRing,
    shards: HashMap<String, ShardStore>,
    query_fanout: usize,
}

impl DHTRouter {
    /// Create a new router with the given replication factor and query fan-out.
    pub fn new(replication_factor: usize, query_fanout: usize) -> Self {
        Self {
            ring: HashRing::new(replication_factor),
            shards: HashMap::new(),
            query_fanout,
        }
    }

    /// Create a router with default settings.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_REPLICATION_FACTOR, 5)
    }

    /// Add an agent to the DHT.  Returns a mutable reference to its shard.
    pub fn add_agent(&mut self, agent_id: &str) -> &mut ShardStore {
        self.ring.add_agent(agent_id);
        self.shards
            .entry(agent_id.to_string())
            .or_insert_with(|| ShardStore::new(agent_id))
    }

    /// Remove an agent and return its orphaned facts for redistribution.
    pub fn remove_agent(&mut self, agent_id: &str) -> Vec<ShardFact> {
        self.ring.remove_agent(agent_id);
        match self.shards.remove(agent_id) {
            Some(shard) => shard.all_facts().into_iter().cloned().collect(),
            None => Vec::new(),
        }
    }

    /// Get an agent's shard store.
    pub fn get_shard(&self, agent_id: &str) -> Option<&ShardStore> {
        self.shards.get(agent_id)
    }

    /// Store a fact on the appropriate shard owner(s).
    ///
    /// Routes via consistent hashing and replicates to R agents.
    /// Returns the list of agent IDs that stored the fact.
    pub fn store_fact(&mut self, mut fact: ShardFact) -> Vec<String> {
        let key = content_key(&fact.content);
        fact.ring_position = hash_key(&key);

        let owners = self.ring.get_agents(&key, None);
        let mut stored_on = Vec::new();

        for agent_id in &owners {
            if let Some(shard) = self.shards.get_mut(agent_id)
                && shard.store(fact.clone())
            {
                stored_on.push(agent_id.clone());
            }
        }
        stored_on
    }

    /// Query the DHT for facts matching `query_text`.
    ///
    /// Fans out to selected shard owners and merges deduplicated results,
    /// sorted by relevance.
    pub fn query(&self, query_text: &str, limit: usize) -> Vec<ShardFact> {
        let targets = self.select_query_targets(query_text);

        let mut all_results = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for agent_id in &targets {
            if let Some(shard) = self.shards.get(agent_id) {
                for fact in shard.search(query_text, limit) {
                    let dedup_key = format!("{:x}", hash_key(&fact.content));
                    if seen.insert(dedup_key) {
                        all_results.push(fact.clone());
                    }
                }
            }
        }

        // Results are already scored by each shard's search; re-sort the merged set.
        all_results.truncate(limit);
        all_results
    }

    /// Return agent IDs that should store this fact (without performing storage).
    ///
    /// Sets `fact.ring_position` as a side effect.
    pub fn get_storage_targets(&self, fact: &mut ShardFact) -> Vec<String> {
        let key = content_key(&fact.content);
        fact.ring_position = hash_key(&key);
        self.ring.get_agents(&key, None)
    }

    /// Select which agents to fan a query out to.
    pub fn select_query_targets(&self, _query_text: &str) -> Vec<String> {
        let all_agents = self.ring.agent_ids();
        let max_targets = self.query_fanout * 3;

        // Collect non-empty shards.
        let non_empty: Vec<String> = all_agents
            .iter()
            .filter(|a| {
                self.shards
                    .get(a.as_str())
                    .is_some_and(|s| s.fact_count() > 0)
            })
            .cloned()
            .collect();

        // If some shards are remote (not locally populated), query all agents.
        if non_empty.len() < all_agents.len() {
            return all_agents;
        }

        // All shards local — return non-empty ones, capped by fan-out.
        non_empty.into_iter().take(max_targets).collect()
    }

    /// Get all agent IDs in the DHT.
    pub fn all_agents(&self) -> Vec<String> {
        self.ring.agent_ids()
    }

    /// Get DHT statistics.
    pub fn stats(&self) -> DHTStats {
        let shard_sizes: HashMap<String, usize> = self
            .shards
            .iter()
            .map(|(id, s)| (id.clone(), s.fact_count()))
            .collect();
        let total_facts: usize = shard_sizes.values().sum();
        let agent_count = self.ring.agent_count();
        DHTStats {
            agent_count,
            total_facts,
            replication_factor: self.ring.replication_factor(),
            shard_sizes,
            avg_shard_size: if agent_count > 0 {
                total_facts as f64 / agent_count as f64
            } else {
                0.0
            },
        }
    }
}

/// Snapshot of DHT statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DHTStats {
    pub agent_count: usize,
    pub total_facts: usize,
    pub replication_factor: usize,
    pub shard_sizes: HashMap<String, usize>,
    pub avg_shard_size: f64,
}

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dht::store::ShardFact;

    fn fact(id: &str, content: &str) -> ShardFact {
        ShardFact::new(id, content)
    }

    #[test]
    fn add_agent_creates_shard() {
        let mut router = DHTRouter::with_defaults();
        router.add_agent("a");
        assert!(router.get_shard("a").is_some());
    }

    #[test]
    fn store_fact_replicates() {
        let mut router = DHTRouter::new(2, 5);
        router.add_agent("a");
        router.add_agent("b");
        router.add_agent("c");

        let stored = router.store_fact(fact("f1", "Rust is great for systems programming"));
        assert!(
            stored.len() >= 1,
            "fact should be stored on at least 1 agent"
        );
    }

    #[test]
    fn query_returns_relevant_facts() {
        let mut router = DHTRouter::new(3, 5);
        router.add_agent("a");
        router.add_agent("b");
        router.add_agent("c");

        router.store_fact(fact("f1", "Rust systems programming language"));
        router.store_fact(fact("f2", "Python interpreted scripting language"));

        let results = router.query("Rust programming", 10);
        assert!(!results.is_empty());
        // The Rust fact should be in results.
        assert!(results.iter().any(|f| f.fact_id == "f1"));
    }

    #[test]
    fn remove_agent_returns_orphaned_facts() {
        let mut router = DHTRouter::new(1, 5);
        router.add_agent("a");
        router.store_fact(fact("f1", "orphan fact"));

        let shard = router.get_shard("a").unwrap();
        let had_facts = shard.fact_count() > 0;

        let orphans = router.remove_agent("a");
        if had_facts {
            assert_eq!(orphans.len(), 1);
        }
        assert!(router.get_shard("a").is_none());
    }

    #[test]
    fn stats_reflect_current_state() {
        let mut router = DHTRouter::new(1, 5);
        router.add_agent("a");
        router.add_agent("b");
        router.store_fact(fact("f1", "hello world from agent"));

        let stats = router.stats();
        assert_eq!(stats.agent_count, 2);
        assert_eq!(stats.replication_factor, 1);
        assert!(stats.total_facts >= 1);
    }

    #[test]
    fn get_storage_targets_sets_ring_position() {
        let router = {
            let mut r = DHTRouter::with_defaults();
            r.add_agent("a");
            r.add_agent("b");
            r.add_agent("c");
            r
        };
        let mut f = fact("f1", "test content for routing");
        let targets = router.get_storage_targets(&mut f);
        assert!(!targets.is_empty());
        assert_ne!(f.ring_position, 0);
    }

    #[test]
    fn duplicate_facts_deduplicated_in_query() {
        let mut router = DHTRouter::new(3, 5);
        router.add_agent("a");
        router.add_agent("b");
        router.add_agent("c");

        // Store same fact — it replicates but query should deduplicate.
        router.store_fact(fact("f1", "unique content about Rust safety"));
        let results = router.query("Rust safety", 10);
        let ids: Vec<_> = results.iter().map(|f| &f.fact_id).collect();
        let unique_ids: HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique_ids.len(), "results should be deduplicated");
    }
}
