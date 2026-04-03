use crate::error::Result;
use crate::graph::HiveGraph;
use crate::models::HiveFact;

/// Policy that decides when a fact should be promoted or broadcast.
pub trait PromotionPolicy: Send + Sync {
    /// Whether the fact should be promoted for the given agent.
    fn should_promote(&self, fact: &HiveFact, agent_id: &str) -> bool;

    /// Whether the fact should be broadcast to all agents.
    fn should_broadcast(&self, fact: &HiveFact) -> bool;
}

/// Threshold-based promotion policy.
pub struct DefaultPromotionPolicy {
    pub promote_threshold: f64,
    pub broadcast_threshold: f64,
}

impl Default for DefaultPromotionPolicy {
    fn default() -> Self {
        Self {
            promote_threshold: 0.7,
            broadcast_threshold: 0.9,
        }
    }
}

impl PromotionPolicy for DefaultPromotionPolicy {
    fn should_promote(&self, fact: &HiveFact, _agent_id: &str) -> bool {
        fact.confidence >= self.promote_threshold
    }

    fn should_broadcast(&self, fact: &HiveFact) -> bool {
        fact.confidence >= self.broadcast_threshold
    }
}

/// Top-level orchestrator that wires the graph, bus, and gossip layers.
pub struct HiveMindOrchestrator {
    graph: HiveGraph,
    policy: Box<dyn PromotionPolicy>,
}

impl HiveMindOrchestrator {
    /// Create an orchestrator with a custom promotion policy.
    pub fn new(policy: Box<dyn PromotionPolicy>) -> Self {
        Self {
            graph: HiveGraph::new(),
            policy,
        }
    }

    /// Create an orchestrator using [`DefaultPromotionPolicy`].
    pub fn with_default_policy() -> Self {
        Self::new(Box::new(DefaultPromotionPolicy::default()))
    }

    /// Store a new fact in the orchestrator's graph.
    pub fn store_fact(
        &mut self,
        concept: &str,
        content: &str,
        confidence: f64,
        source_id: &str,
    ) -> Result<String> {
        self.graph
            .store_fact(concept, content, confidence, source_id, vec![])
    }

    /// Query facts by concept.
    pub fn query(&self, concept: &str) -> Result<Vec<HiveFact>> {
        self.graph.query_facts(concept, 0.0, 100)
    }

    /// Attempt to promote a fact for a specific agent.
    pub fn promote(&mut self, fact_id: &str, agent_id: &str) -> Result<bool> {
        match self.graph.get_fact(fact_id)? {
            Some(fact) => Ok(self.policy.should_promote(&fact, agent_id)),
            None => Ok(false),
        }
    }

    /// Return a reference to the active promotion policy.
    pub fn policy(&self) -> &dyn PromotionPolicy {
        &*self.policy
    }
}
