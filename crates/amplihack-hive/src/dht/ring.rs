use super::{VIRTUAL_NODES_PER_AGENT, hash_key};

/// Consistent hash ring for distributing facts across agents.
///
/// Uses virtual nodes for even distribution.  Each agent gets
/// [`VIRTUAL_NODES_PER_AGENT`] positions on the ring.
#[derive(Debug)]
pub struct HashRing {
    ring: Vec<u32>,
    ring_to_agent: std::collections::HashMap<u32, String>,
    agent_positions: std::collections::HashMap<String, Vec<u32>>,
    replication_factor: usize,
}

impl HashRing {
    /// Create a new ring with the given replication factor.
    pub fn new(replication_factor: usize) -> Self {
        Self {
            ring: Vec::new(),
            ring_to_agent: std::collections::HashMap::new(),
            agent_positions: std::collections::HashMap::new(),
            replication_factor,
        }
    }

    /// Return the replication factor.
    pub fn replication_factor(&self) -> usize {
        self.replication_factor
    }

    /// Add an agent to the ring with virtual nodes.
    pub fn add_agent(&mut self, agent_id: &str) {
        if self.agent_positions.contains_key(agent_id) {
            return;
        }
        let mut positions = Vec::with_capacity(VIRTUAL_NODES_PER_AGENT);
        for i in 0..VIRTUAL_NODES_PER_AGENT {
            let vnode_key = format!("{agent_id}:vnode:{i}");
            let pos = hash_key(&vnode_key);
            self.ring_to_agent.insert(pos, agent_id.to_string());
            positions.push(pos);
        }
        self.agent_positions.insert(agent_id.to_string(), positions);
        self.ring = self.ring_to_agent.keys().copied().collect();
        self.ring.sort_unstable();
    }

    /// Remove an agent and its virtual nodes from the ring.
    pub fn remove_agent(&mut self, agent_id: &str) {
        if let Some(positions) = self.agent_positions.remove(agent_id) {
            for pos in positions {
                self.ring_to_agent.remove(&pos);
            }
            self.ring = self.ring_to_agent.keys().copied().collect();
            self.ring.sort_unstable();
        }
    }

    /// Find the `n` agents responsible for `key` (clockwise from hash).
    ///
    /// Returns up to `min(n, unique_agent_count)` distinct agent IDs.
    pub fn get_agents(&self, key: &str, n: Option<usize>) -> Vec<String> {
        let n = n.unwrap_or(self.replication_factor);
        if self.ring.is_empty() {
            return Vec::new();
        }

        let pos = hash_key(key);
        let idx = match self.ring.binary_search(&pos) {
            Ok(i) => i + 1,
            Err(i) => i,
        };

        let ring_len = self.ring.len();
        let mut agents_seen = Vec::new();
        let mut unique = std::collections::HashSet::new();

        for offset in 0..ring_len {
            let ring_pos = self.ring[(idx + offset) % ring_len];
            if let Some(agent) = self.ring_to_agent.get(&ring_pos)
                && unique.insert(agent.clone())
            {
                agents_seen.push(agent.clone());
                if agents_seen.len() >= n {
                    break;
                }
            }
        }
        agents_seen
    }

    /// Get the primary (first) agent responsible for a key.
    pub fn get_primary_agent(&self, key: &str) -> Option<String> {
        self.get_agents(key, Some(1)).into_iter().next()
    }

    /// Return the number of agents on the ring.
    pub fn agent_count(&self) -> usize {
        self.agent_positions.len()
    }

    /// Return all agent IDs currently on the ring.
    pub fn agent_ids(&self) -> Vec<String> {
        self.agent_positions.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dht::DEFAULT_REPLICATION_FACTOR;

    #[test]
    fn add_and_lookup() {
        let mut ring = HashRing::new(DEFAULT_REPLICATION_FACTOR);
        ring.add_agent("agent-1");
        ring.add_agent("agent-2");
        ring.add_agent("agent-3");

        let agents = ring.get_agents("some-key", None);
        assert_eq!(agents.len(), DEFAULT_REPLICATION_FACTOR);
        // All returned agents are distinct.
        let unique: std::collections::HashSet<_> = agents.iter().collect();
        assert_eq!(unique.len(), DEFAULT_REPLICATION_FACTOR);
    }

    #[test]
    fn primary_agent_is_deterministic() {
        let mut ring = HashRing::new(1);
        ring.add_agent("a");
        ring.add_agent("b");
        let first = ring.get_primary_agent("key");
        let second = ring.get_primary_agent("key");
        assert_eq!(first, second);
    }

    #[test]
    fn remove_agent_clears_positions() {
        let mut ring = HashRing::new(1);
        ring.add_agent("a");
        ring.add_agent("b");
        assert_eq!(ring.agent_count(), 2);
        ring.remove_agent("a");
        assert_eq!(ring.agent_count(), 1);
        // All lookups now go to "b".
        let agent = ring.get_primary_agent("any-key").unwrap();
        assert_eq!(agent, "b");
    }

    #[test]
    fn empty_ring_returns_empty() {
        let ring = HashRing::new(3);
        assert!(ring.get_agents("key", None).is_empty());
        assert!(ring.get_primary_agent("key").is_none());
    }

    #[test]
    fn duplicate_add_is_idempotent() {
        let mut ring = HashRing::new(1);
        ring.add_agent("a");
        ring.add_agent("a");
        assert_eq!(ring.agent_count(), 1);
    }

    #[test]
    fn distribution_is_reasonably_even() {
        let mut ring = HashRing::new(1);
        for i in 0..4 {
            ring.add_agent(&format!("agent-{i}"));
        }
        let mut counts = std::collections::HashMap::<String, usize>::new();
        for i in 0..1000 {
            if let Some(agent) = ring.get_primary_agent(&format!("key-{i}")) {
                *counts.entry(agent).or_default() += 1;
            }
        }
        // Each of 4 agents should get roughly 250 keys.  Allow wide tolerance.
        for count in counts.values() {
            assert!(*count > 100, "agent got only {count} keys out of 1000");
            assert!(*count < 500, "agent got {count} keys out of 1000");
        }
    }
}
