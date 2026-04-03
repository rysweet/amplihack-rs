use std::collections::HashSet;

use crate::error::Result;
use crate::models::{GossipConfig, GossipMessage, HiveFact, MergeResult};

/// Epidemic gossip protocol for propagating facts across hive nodes.
pub struct GossipProtocol {
    config: GossipConfig,
    round: u64,
    node_id: String,
}

impl GossipProtocol {
    /// Create a new gossip protocol instance for the given node.
    pub fn new(node_id: String, config: GossipConfig) -> Self {
        Self {
            config,
            round: 0,
            node_id,
        }
    }

    /// Execute one gossip round, merging local and peer facts.
    pub fn run_gossip_round(
        &mut self,
        local_facts: &[HiveFact],
        peer_facts: &[HiveFact],
    ) -> Result<MergeResult> {
        let local_ids: HashSet<&str> = local_facts.iter().map(|f| f.fact_id.as_str()).collect();
        let mut accepted = Vec::with_capacity(peer_facts.len());
        let mut rejected = Vec::new();
        let mut conflicts = Vec::new();

        for fact in peer_facts {
            if fact.confidence < self.config.min_confidence {
                rejected.push(fact.fact_id.clone());
            } else if local_ids.contains(fact.fact_id.as_str()) {
                conflicts.push(fact.fact_id.clone());
            } else {
                accepted.push(fact.fact_id.clone());
            }
        }

        self.round += 1;
        Ok(MergeResult {
            accepted,
            rejected,
            conflicts,
        })
    }

    /// Build a [`GossipMessage`] from a set of facts.
    pub fn prepare_message(&self, facts: Vec<HiveFact>) -> GossipMessage {
        GossipMessage {
            facts,
            source_id: self.node_id.clone(),
            round: self.round,
        }
    }

    /// Merge an incoming gossip message into local state.
    pub fn merge_incoming(&mut self, message: GossipMessage) -> Result<MergeResult> {
        let mut accepted = Vec::new();
        let mut rejected = Vec::new();

        for fact in &message.facts {
            if fact.confidence < self.config.min_confidence {
                rejected.push(fact.fact_id.clone());
            } else {
                accepted.push(fact.fact_id.clone());
            }
        }

        self.round += 1;
        Ok(MergeResult {
            accepted,
            rejected,
            conflicts: vec![],
        })
    }

    /// Return the current gossip round number.
    pub fn current_round(&self) -> u64 {
        self.round
    }

    /// Return this node's identifier.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Return the gossip configuration.
    pub fn config(&self) -> &GossipConfig {
        &self.config
    }

    /// Select a subset of peers to gossip with based on fanout.
    ///
    /// Uses a round-based shuffle to avoid always picking the same first-N peers.
    pub fn select_peers(&self, all_peers: &[String]) -> Vec<String> {
        let n = self.config.fanout.min(all_peers.len());
        if n == 0 || all_peers.is_empty() {
            return Vec::new();
        }
        let mut peers: Vec<String> = all_peers.to_vec();
        // Simple deterministic shuffle seeded by the current round number.
        // Fisher-Yates using a basic LCG so we don't need the `rand` crate.
        let mut rng = self.round.wrapping_mul(6364136223846793005).wrapping_add(1);
        for i in (1..peers.len()).rev() {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let j = (rng >> 33) as usize % (i + 1);
            peers.swap(i, j);
        }
        peers.truncate(n);
        peers
    }
}
