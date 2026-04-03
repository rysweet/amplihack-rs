use std::collections::{HashMap, HashSet};

use crate::error::Result;
use crate::event_bus::{EventBus, LocalEventBus};
use crate::gossip::GossipProtocol;
use crate::graph::HiveGraph;
use crate::models::{
    BusEvent, HiveFact, DEFAULT_BROADCAST_THRESHOLD, DEFAULT_CONFIDENCE_GATE,
    GOSSIP_MIN_CONFIDENCE, PEER_CONFIDENCE_DISCOUNT,
};

/// Policy that decides when a fact should be promoted, broadcast, or gossiped.
pub trait PromotionPolicy: Send + Sync {
    fn should_promote(&self, fact: &HiveFact, agent_id: &str) -> bool;
    fn should_broadcast(&self, fact: &HiveFact) -> bool;
    fn should_gossip(&self, fact: &HiveFact) -> bool;
}

/// Threshold-based promotion policy using ported Python constants.
pub struct DefaultPromotionPolicy {
    pub promote_threshold: f64,
    pub broadcast_threshold: f64,
    pub gossip_threshold: f64,
}

impl Default for DefaultPromotionPolicy {
    fn default() -> Self {
        Self {
            promote_threshold: DEFAULT_CONFIDENCE_GATE,
            broadcast_threshold: DEFAULT_BROADCAST_THRESHOLD,
            gossip_threshold: GOSSIP_MIN_CONFIDENCE,
        }
    }
}

impl PromotionPolicy for DefaultPromotionPolicy {
    fn should_promote(&self, fact: &HiveFact, _agent_id: &str) -> bool {
        fact.status != "retracted" && fact.confidence >= self.promote_threshold
    }
    fn should_broadcast(&self, fact: &HiveFact) -> bool {
        fact.status != "retracted" && fact.confidence >= self.broadcast_threshold
    }
    fn should_gossip(&self, fact: &HiveFact) -> bool {
        fact.status != "retracted" && fact.confidence >= self.gossip_threshold
    }
}

/// Result of the 4-layer store-and-promote pipeline.
#[derive(Clone, Debug)]
pub struct PromotionResult {
    pub fact_id: String,
    pub stored: bool,
    pub promoted: bool,
    pub broadcast: bool,
    pub gossiped: bool,
}

/// Top-level orchestrator that wires the graph, bus, and gossip layers.
pub struct HiveMindOrchestrator {
    graph: HiveGraph,
    policy: Box<dyn PromotionPolicy>,
    agent_id: String,
    bus: LocalEventBus,
    peers: Vec<HiveGraph>,
    gossip: Option<GossipProtocol>,
}

impl HiveMindOrchestrator {
    pub fn new(policy: Box<dyn PromotionPolicy>) -> Self {
        Self {
            graph: HiveGraph::new(), policy, agent_id: String::new(),
            bus: LocalEventBus::new(), peers: Vec::new(), gossip: None,
        }
    }

    pub fn with_default_policy() -> Self {
        Self::new(Box::new(DefaultPromotionPolicy::default()))
    }

    pub fn with_agent_id(mut self, id: impl Into<String>) -> Self {
        self.agent_id = id.into();
        self
    }

    // ── Legacy API (preserved) ─────────────────────────────────

    pub fn store_fact(&mut self, concept: &str, content: &str, confidence: f64, source_id: &str) -> Result<String> {
        self.graph.store_fact(concept, content, confidence, source_id, vec![])
    }

    pub fn query(&self, concept: &str) -> Result<Vec<HiveFact>> {
        self.graph.query_facts(concept, 0.0, 100)
    }

    pub fn promote(&mut self, fact_id: &str, agent_id: &str) -> Result<bool> {
        match self.graph.get_fact(fact_id)? {
            Some(fact) => Ok(self.policy.should_promote(&fact, agent_id)),
            None => Ok(false),
        }
    }

    pub fn policy(&self) -> &dyn PromotionPolicy { &*self.policy }

    // ── New API ────────────────────────────────────────────────

    /// 4-layer pipeline: store → promote → broadcast → gossip.
    pub fn store_and_promote(
        &mut self, concept: &str, content: &str, confidence: f64, source_id: &str,
    ) -> Result<PromotionResult> {
        let fact_id = self.graph.store_fact(concept, content, confidence, source_id, vec![])?;
        let fact = self.graph.get_fact(&fact_id)?.expect("just stored");
        let promoted = self.policy.should_promote(&fact, source_id);
        let broadcast = promoted && self.policy.should_broadcast(&fact);
        if broadcast {
            let ev = crate::models::make_event("fact.broadcast", source_id,
                serde_json::json!({"fact_id": fact_id, "concept": concept, "content": content, "confidence": confidence}));
            let _ = self.bus.publish(ev);
        }
        let gossiped = promoted && self.policy.should_gossip(&fact);
        Ok(PromotionResult { fact_id, stored: true, promoted, broadcast, gossiped })
    }

    /// Query local graph + all peers, dedup by content hash, RRF merge.
    pub fn query_unified(&self, concept: &str, min_confidence: f64, limit: usize) -> Result<Vec<HiveFact>> {
        let mut all: Vec<HiveFact> = self.graph.query_facts(concept, min_confidence, limit * 2)?;
        for (rank, peer) in self.peers.iter().enumerate() {
            let peer_facts = peer.query_facts(concept, min_confidence, limit * 2)?;
            for mut pf in peer_facts {
                pf.confidence *= PEER_CONFIDENCE_DISCOUNT;
                // Simple RRF-style boost based on peer rank
                pf.confidence *= 1.0 / (1.0 + rank as f64);
                pf.confidence = pf.confidence.clamp(0.0, 1.0);
                all.push(pf);
            }
        }
        // Dedup by content
        let mut seen: HashSet<String> = HashSet::new();
        all.retain(|f| seen.insert(f.content.clone()));
        all.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        all.truncate(limit);
        Ok(all)
    }

    /// Incorporate a peer event, applying confidence discount.
    pub fn process_event(&mut self, event: &BusEvent) -> Result<Option<String>> {
        if let Some(concept) = event.payload.get("concept").and_then(|v| v.as_str()) {
            let content = event.payload.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let raw_conf = event.payload.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.5);
            let discounted = (raw_conf * PEER_CONFIDENCE_DISCOUNT).clamp(0.0, 1.0);
            let id = self.graph.store_fact(concept, content, discounted, &event.source_id, vec![])?;
            return Ok(Some(id));
        }
        Ok(None)
    }

    /// Execute one gossip round across all peers.
    pub fn run_gossip_round(&mut self) -> Result<Vec<String>> {
        let gossip = match &mut self.gossip { Some(g) => g, None => return Ok(vec![]) };
        let local_facts = self.graph.all_facts();
        let mut accepted_ids = Vec::new();
        let peer_graphs: Vec<Vec<HiveFact>> = self.peers.iter().map(|p| p.all_facts()).collect();
        for peer_facts in &peer_graphs {
            let result = gossip.run_gossip_round(&local_facts, peer_facts)?;
            for fid in &result.accepted {
                if let Some(pf) = peer_facts.iter().find(|f| f.fact_id == *fid) {
                    let mut imported = pf.clone();
                    imported.confidence = (imported.confidence * PEER_CONFIDENCE_DISCOUNT).clamp(0.0, 1.0);
                    let new_id = self.graph.store_fact(
                        &imported.concept, &imported.content, imported.confidence, &imported.source_id, imported.tags.clone(),
                    )?;
                    accepted_ids.push(new_id);
                }
            }
        }
        Ok(accepted_ids)
    }

    /// Poll and process all pending events from the bus.
    pub fn drain_events(&mut self) -> Result<Vec<BusEvent>> {
        let agent = if self.agent_id.is_empty() { "orchestrator" } else { &self.agent_id };
        self.bus.poll(agent)
    }

    pub fn add_peer(&mut self, peer: HiveGraph) { self.peers.push(peer); }
    pub fn peer_count(&self) -> usize { self.peers.len() }
    pub fn set_gossip(&mut self, g: GossipProtocol) { self.gossip = Some(g); }
    pub fn agent_id(&self) -> &str { &self.agent_id }

    pub fn close(&mut self) -> Result<()> {
        let _ = self.bus.close();
        self.peers.clear();
        self.gossip = None;
        Ok(())
    }

    /// Expose the internal graph (for testing).
    pub fn graph(&self) -> &HiveGraph { &self.graph }
    /// Expose mutable graph.
    pub fn graph_mut(&mut self) -> &mut HiveGraph { &mut self.graph }
    /// Expose the bus (for testing).
    pub fn bus_mut(&mut self) -> &mut LocalEventBus { &mut self.bus }
}
