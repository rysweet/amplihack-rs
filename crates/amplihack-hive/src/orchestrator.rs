use std::collections::HashSet;

use crate::error::{HiveError, Result};
use crate::event_bus::{EventBus, LocalEventBus};
use crate::gossip::GossipProtocol;
use crate::graph::HiveGraph;
use crate::models::{BusEvent, HiveFact, PEER_CONFIDENCE_DISCOUNT};

/// Policy that decides when a fact should be promoted, broadcast, or gossiped.
pub trait PromotionPolicy: Send + Sync {
    /// Whether the fact should be promoted for the given agent.
    fn should_promote(&self, fact: &HiveFact, agent_id: &str) -> bool;
    /// Whether the fact should be broadcast to all agents.
    fn should_broadcast(&self, fact: &HiveFact) -> bool;
    /// Whether the fact should be included in gossip rounds.
    fn should_gossip(&self, fact: &HiveFact) -> bool;
}

/// Threshold-based promotion policy.
pub struct DefaultPromotionPolicy {
    pub promote_threshold: f64,
    pub broadcast_threshold: f64,
    pub gossip_threshold: f64,
}

impl Default for DefaultPromotionPolicy {
    fn default() -> Self {
        Self {
            promote_threshold: 0.7,
            broadcast_threshold: 0.9,
            gossip_threshold: 0.5,
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

/// Outcome of a store-and-promote operation.
#[derive(Clone, Debug)]
pub struct PromotionResult {
    pub fact_id: String,
    pub promoted: bool,
    pub broadcast: bool,
}

/// Result of processing a single peer event.
#[derive(Clone, Debug)]
pub struct ProcessEventResult {
    pub incorporated: bool,
    pub fact_id: Option<String>,
    pub reason: String,
}

/// Result of a gossip round.
#[derive(Clone, Debug)]
pub struct GossipRoundResult {
    pub peers_contacted: usize,
    pub skipped: Option<String>,
}

/// Top-level orchestrator that wires the graph, bus, and gossip layers.
pub struct HiveMindOrchestrator {
    graph: HiveGraph,
    policy: Box<dyn PromotionPolicy>,
    agent_id: String,
    bus: LocalEventBus,
    peers: Vec<HiveMindOrchestrator>,
    gossip: Option<GossipProtocol>,
    pending_events: Vec<BusEvent>,
    closed: bool,
}

impl HiveMindOrchestrator {
    /// Create an orchestrator with a custom promotion policy.
    pub fn new(policy: Box<dyn PromotionPolicy>) -> Self {
        Self {
            graph: HiveGraph::new(),
            policy,
            agent_id: String::new(),
            bus: LocalEventBus::new(),
            peers: Vec::new(),
            gossip: None,
            pending_events: Vec::new(),
            closed: false,
        }
    }

    /// Create an orchestrator using [`DefaultPromotionPolicy`].
    pub fn with_default_policy() -> Self {
        Self::new(Box::new(DefaultPromotionPolicy::default()))
    }

    /// Set the agent id for this orchestrator (builder pattern).
    pub fn with_agent_id(mut self, agent_id: String) -> Self {
        self.agent_id = agent_id;
        self
    }

    /// Store a new fact in the orchestrator's graph.
    pub fn store_fact(
        &mut self, concept: &str, content: &str, confidence: f64, source_id: &str,
    ) -> Result<String> {
        self.graph.store_fact(concept, content, confidence, source_id, vec![])
    }

    /// Store a fact and decide whether to promote/broadcast it.
    pub fn store_and_promote(
        &mut self, concept: &str, content: &str, confidence: f64, source_id: &str,
    ) -> Result<PromotionResult> {
        let fact_id = self.graph.store_fact(concept, content, confidence, source_id, vec![])?;
        let fact = self.graph.get_fact(&fact_id)?
            .ok_or_else(|| HiveError::FactNotFound(fact_id.clone()))?;
        let promoted = self.policy.should_promote(&fact, &self.agent_id);
        let broadcast = self.policy.should_broadcast(&fact);
        Ok(PromotionResult { fact_id, promoted, broadcast })
    }

    /// Query facts by concept.
    pub fn query(&self, concept: &str) -> Result<Vec<HiveFact>> {
        self.graph.query_facts(concept, 0.0, 100)
    }

    /// Query facts from this orchestrator and all peers, with dedup.
    pub fn query_unified(&self, concept: &str) -> Result<Vec<HiveFact>> {
        let mut results = self.query(concept)?;
        for peer in &self.peers {
            results.extend(peer.query(concept)?);
        }
        let mut seen = HashSet::new();
        results.retain(|f| seen.insert(f.content.clone()));
        results.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal));
        Ok(results)
    }

    /// Attempt to promote a fact for a specific agent.
    pub fn promote(&mut self, fact_id: &str, agent_id: &str) -> Result<bool> {
        match self.graph.get_fact(fact_id)? {
            Some(fact) => Ok(self.policy.should_promote(&fact, agent_id)),
            None => Ok(false),
        }
    }

    /// Process an incoming bus event with peer confidence discount.
    pub fn process_event(&mut self, event: &BusEvent) -> ProcessEventResult {
        let event_type = event.event_type();
        if event_type != "FACT_PROMOTED" {
            self.pending_events.push(event.clone());
            return ProcessEventResult {
                incorporated: false, fact_id: None,
                reason: "not a FACT_PROMOTED event".into(),
            };
        }
        if event.source_agent() == self.agent_id {
            return ProcessEventResult {
                incorporated: false, fact_id: None,
                reason: "self-published event (skipped)".into(),
            };
        }
        let data = &event.payload;
        let concept = data.get("concept").and_then(|v| v.as_str()).unwrap_or("");
        let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let confidence = data.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if content.is_empty() || concept.is_empty() {
            return ProcessEventResult {
                incorporated: false, fact_id: None,
                reason: "missing content or concept".into(),
            };
        }
        let discounted = confidence * PEER_CONFIDENCE_DISCOUNT;
        match self.store_fact(concept, content, discounted, event.source_agent()) {
            Ok(id) => ProcessEventResult {
                incorporated: true, fact_id: Some(id),
                reason: "promoted from peer event".into(),
            },
            Err(e) => ProcessEventResult {
                incorporated: false, fact_id: None,
                reason: format!("store failed: {e}"),
            },
        }
    }

    /// Drain pending events, returning one result per event.
    pub fn drain_events(&mut self) -> Vec<ProcessEventResult> {
        let events = std::mem::take(&mut self.pending_events);
        events.iter().map(|e| {
            ProcessEventResult {
                incorporated: false,
                fact_id: None,
                reason: format!("drained: {}", e.event_type()),
            }
        }).collect()
    }

    /// Drain raw pending events without processing.
    pub fn drain_raw_events(&mut self) -> Vec<BusEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Execute one round of Layer 3 gossip dissemination.
    pub fn run_gossip_round(&mut self) -> GossipRoundResult {
        if self.gossip.is_none() {
            return GossipRoundResult {
                peers_contacted: 0, skipped: Some("gossip protocol not configured".into()),
            };
        }
        if self.peers.is_empty() {
            return GossipRoundResult {
                peers_contacted: 0, skipped: Some("no peers registered".into()),
            };
        }
        let gossip = self.gossip.as_mut().unwrap();
        let local_facts = self.graph.all_facts();
        let mut contacted = 0;
        for peer in &self.peers {
            let peer_facts = peer.graph.all_facts();
            if gossip.run_gossip_round(&local_facts, &peer_facts).is_ok() {
                contacted += 1;
            }
        }
        GossipRoundResult { peers_contacted: contacted, skipped: None }
    }

    /// Return all facts from this orchestrator's graph.
    pub fn all_facts(&self) -> Vec<HiveFact> { self.graph.all_facts() }

    /// Add a peer orchestrator for gossip.
    pub fn add_peer(&mut self, peer: HiveMindOrchestrator) { self.peers.push(peer); }

    /// Return the number of peers.
    pub fn peer_count(&self) -> usize { self.peers.len() }

    /// Collect all facts from all peers.
    pub fn all_peer_facts(&self) -> Vec<HiveFact> {
        self.peers.iter().flat_map(|p| p.all_facts()).collect()
    }

    /// Return a reference to the active promotion policy.
    pub fn policy(&self) -> &dyn PromotionPolicy { &*self.policy }

    /// Return the agent id.
    pub fn agent_id(&self) -> &str { &self.agent_id }

    /// Close this orchestrator.
    pub fn close(&mut self) -> Result<()> {
        self.closed = true;
        self.pending_events.clear();
        self.peers.clear();
        self.bus.close()?;
        Ok(())
    }

    pub fn is_closed(&self) -> bool { self.closed }
    pub fn graph(&self) -> &HiveGraph { &self.graph }
    pub fn bus_mut(&mut self) -> &mut LocalEventBus { &mut self.bus }
    pub fn set_gossip(&mut self, gossip: GossipProtocol) { self.gossip = Some(gossip); }
}
