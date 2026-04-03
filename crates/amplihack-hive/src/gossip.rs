use std::collections::HashSet;

use crate::error::Result;
use crate::graph::HiveGraph;
use crate::models::{GossipConfig, GossipMessage, HiveFact, MergeResult};

pub const GOSSIP_TAG_PREFIX: &str = "gossip:";

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
    /// Content-based deduplication: if peer fact content matches any local fact, treat as conflict.
    pub fn run_gossip_round(
        &mut self,
        local_facts: &[HiveFact],
        peer_facts: &[HiveFact],
    ) -> Result<MergeResult> {
        let local_ids: HashSet<&str> = local_facts.iter().map(|f| f.fact_id.as_str()).collect();
        let local_contents: HashSet<&str> =
            local_facts.iter().map(|f| f.content.as_str()).collect();
        let mut accepted = Vec::with_capacity(peer_facts.len());
        let mut rejected = Vec::new();
        let mut conflicts = Vec::new();

        for fact in peer_facts {
            if fact.confidence < self.config.min_confidence {
                rejected.push(fact.fact_id.clone());
            } else if local_ids.contains(fact.fact_id.as_str())
                || local_contents.contains(fact.content.as_str())
            {
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

    /// Build a [`GossipMessage`] from a set of facts, tagging them with the gossip prefix.
    pub fn prepare_message(&self, facts: Vec<HiveFact>) -> GossipMessage {
        let tagged: Vec<HiveFact> = facts
            .into_iter()
            .map(|mut f| {
                f.tags
                    .push(format!("{}{}", GOSSIP_TAG_PREFIX, self.node_id));
                f
            })
            .collect();
        GossipMessage {
            facts: tagged,
            source_id: self.node_id.clone(),
            round: self.round,
        }
    }

    /// Merge an incoming gossip message into local state.
    ///
    /// NOTE: conflicts will always be empty. This method only has access to the
    /// incoming message and cannot compare against local facts. Use
    /// [`run_gossip_round()`](Self::run_gossip_round) for conflict detection.
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

    /// Select peers weighted by sum of active agent trust scores per hive.
    /// Uses same LCG-seeded approach. Floors trust sum at 0.1.
    pub fn select_peers_weighted(
        &self,
        peer_hives: &[&HiveGraph],
        exclude_id: Option<&str>,
    ) -> Vec<String> {
        let candidates: Vec<(&HiveGraph, f64)> = peer_hives
            .iter()
            .filter(|h| {
                if let Some(exc) = exclude_id {
                    h.hive_id() != exc
                } else {
                    true
                }
            })
            .map(|h| {
                let trust_sum: f64 = h
                    .all_facts()
                    .iter()
                    .map(|_| 0.0_f64)
                    .sum::<f64>(); // placeholder
                let agent_trust: f64 = h
                    .list_agents(Some("active"))
                    .iter()
                    .map(|a| a.trust)
                    .sum();
                let weight = agent_trust.max(0.1);
                (*h, weight + trust_sum)
            })
            .collect();

        if candidates.is_empty() {
            return Vec::new();
        }

        let n = self.config.fanout.min(candidates.len());
        let total_weight: f64 = candidates.iter().map(|(_, w)| w).sum();
        let mut rng = self.round.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut selected = Vec::with_capacity(n);
        let mut used: HashSet<usize> = HashSet::new();

        for _ in 0..n {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let r = ((rng >> 33) as f64 / (u32::MAX as f64)) * total_weight;
            let mut cumulative = 0.0;
            let mut pick = 0;
            for (i, (_, w)) in candidates.iter().enumerate() {
                if used.contains(&i) {
                    continue;
                }
                cumulative += w;
                if cumulative >= r {
                    pick = i;
                    break;
                }
                pick = i;
            }
            if !used.contains(&pick) {
                used.insert(pick);
                selected.push(candidates[pick].0.hive_id().to_string());
            }
        }
        selected
    }

    /// Return the top-K facts from a hive by confidence, excluding retracted.
    pub fn get_top_facts(hive: &HiveGraph, top_k: usize) -> Vec<HiveFact> {
        let mut facts: Vec<HiveFact> = hive
            .all_facts()
            .iter()
            .filter(|f| f.status != "retracted")
            .cloned()
            .collect();
        facts.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        facts.truncate(top_k);
        facts
    }
}

/// Compute convergence ratio across multiple hives.
/// |intersection| / |union| of non-retracted fact content.
/// Returns 1.0 for empty or single hive.
pub fn convergence_check(hives: &[&HiveGraph]) -> f64 {
    if hives.len() <= 1 {
        return 1.0;
    }
    let content_sets: Vec<HashSet<String>> = hives
        .iter()
        .map(|h| {
            h.all_facts()
                .iter()
                .filter(|f| f.status != "retracted")
                .map(|f| f.content.clone())
                .collect::<HashSet<String>>()
        })
        .collect();

    let union: HashSet<String> = content_sets.iter().flatten().cloned().collect();
    if union.is_empty() {
        return 1.0;
    }

    let mut intersection = content_sets[0].clone();
    for set in &content_sets[1..] {
        intersection = intersection.intersection(set).cloned().collect();
    }

    intersection.len() as f64 / union.len() as f64
}
