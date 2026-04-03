//! Consistent hash ring for DHT distribution.
//!
//! Matches Python `amplihack/memory/hash_ring.py`:
//! - Virtual nodes (64 per agent) for even distribution
//! - Replication factor support (N agents per key)
//! - Thread-safe; supports dynamic agent join/leave

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

/// Consistent hash ring with virtual nodes.
pub struct HashRing {
    ring: BTreeMap<u64, String>,
    virtual_nodes: u32,
    agents: Vec<String>,
}

impl HashRing {
    /// Create a new hash ring with the given number of virtual nodes per agent.
    pub fn new(virtual_nodes: u32) -> Self {
        Self {
            ring: BTreeMap::new(),
            virtual_nodes,
            agents: Vec::new(),
        }
    }

    /// Default ring with 64 virtual nodes per agent.
    pub fn default_ring() -> Self {
        Self::new(64)
    }

    /// Add an agent to the ring.
    pub fn add_agent(&mut self, agent_id: &str) {
        if self.agents.iter().any(|a| a == agent_id) {
            return;
        }
        let owned = agent_id.to_string();
        for i in 0..self.virtual_nodes {
            let vnode_key = format!("{agent_id}:vnode:{i}");
            let hash = hash_key(&vnode_key);
            self.ring.insert(hash, owned.clone());
        }
        self.agents.push(owned);
    }

    /// Remove an agent from the ring.
    pub fn remove_agent(&mut self, agent_id: &str) {
        self.agents.retain(|a| a != agent_id);
        self.ring.retain(|_, v| v != agent_id);
    }

    /// Get the primary agent responsible for a key.
    pub fn get_primary_agent(&self, key: &str) -> Option<&str> {
        self.get_agents(key, 1).into_iter().next()
    }

    /// Get up to `n` distinct agents responsible for a key.
    /// Returns agents in clockwise order from the key's position.
    pub fn get_agents(&self, key: &str, n: usize) -> Vec<&str> {
        if self.ring.is_empty() {
            return Vec::new();
        }
        let hash = hash_key(key);
        let mut result = Vec::with_capacity(n.min(self.agents.len()));
        let mut seen = std::collections::HashSet::new();

        // Walk clockwise from the key's hash position
        for (_, agent) in self.ring.range(hash..).chain(self.ring.iter()) {
            if seen.insert(agent.as_str()) {
                result.push(agent.as_str());
                if result.len() >= n {
                    break;
                }
            }
        }
        result
    }

    /// Number of agents in the ring.
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// All registered agent IDs.
    pub fn agents(&self) -> &[String] {
        &self.agents
    }

    /// Total virtual nodes in the ring.
    pub fn ring_size(&self) -> usize {
        self.ring.len()
    }
}

fn hash_key(key: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ring_returns_none() {
        let ring = HashRing::default_ring();
        assert!(ring.get_primary_agent("any-key").is_none());
        assert!(ring.get_agents("key", 3).is_empty());
    }

    #[test]
    fn single_agent_always_returned() {
        let mut ring = HashRing::default_ring();
        ring.add_agent("agent-1");
        assert_eq!(ring.get_primary_agent("any-key"), Some("agent-1"));
        assert_eq!(ring.get_primary_agent("other-key"), Some("agent-1"));
    }

    #[test]
    fn multiple_agents_distributed() {
        let mut ring = HashRing::default_ring();
        ring.add_agent("agent-1");
        ring.add_agent("agent-2");
        ring.add_agent("agent-3");

        // With 3 agents and many keys, all should appear
        let mut seen = std::collections::HashSet::new();
        for i in 0..100 {
            if let Some(agent) = ring.get_primary_agent(&format!("key-{i}")) {
                seen.insert(agent.to_string());
            }
        }
        assert_eq!(seen.len(), 3, "all agents should be primary for some key");
    }

    #[test]
    fn replication_returns_distinct_agents() {
        let mut ring = HashRing::default_ring();
        ring.add_agent("a1");
        ring.add_agent("a2");
        ring.add_agent("a3");
        let agents = ring.get_agents("test-key", 3);
        assert_eq!(agents.len(), 3);
        let unique: std::collections::HashSet<_> = agents.iter().collect();
        assert_eq!(unique.len(), 3, "all agents should be distinct");
    }

    #[test]
    fn replication_caps_at_agent_count() {
        let mut ring = HashRing::default_ring();
        ring.add_agent("a1");
        ring.add_agent("a2");
        let agents = ring.get_agents("key", 5);
        assert_eq!(agents.len(), 2, "can't return more agents than exist");
    }

    #[test]
    fn remove_agent_works() {
        let mut ring = HashRing::default_ring();
        ring.add_agent("a1");
        ring.add_agent("a2");
        assert_eq!(ring.agent_count(), 2);
        ring.remove_agent("a1");
        assert_eq!(ring.agent_count(), 1);
        assert_eq!(ring.get_primary_agent("key"), Some("a2"));
    }

    #[test]
    fn duplicate_add_is_idempotent() {
        let mut ring = HashRing::default_ring();
        ring.add_agent("a1");
        ring.add_agent("a1");
        assert_eq!(ring.agent_count(), 1);
        assert_eq!(ring.ring_size(), 64);
    }

    #[test]
    fn consistent_assignment() {
        let mut ring = HashRing::default_ring();
        ring.add_agent("a1");
        ring.add_agent("a2");
        let first = ring.get_primary_agent("stable-key");
        let second = ring.get_primary_agent("stable-key");
        assert_eq!(first, second, "same key should map to same agent");
    }

    #[test]
    fn virtual_nodes_create_ring_entries() {
        let mut ring = HashRing::new(10);
        ring.add_agent("agent-x");
        assert_eq!(ring.ring_size(), 10);
    }
}
