use crate::error::Result;
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
    policy: Box<dyn PromotionPolicy>,
}

impl HiveMindOrchestrator {
    /// Create an orchestrator with a custom promotion policy.
    pub fn new(policy: Box<dyn PromotionPolicy>) -> Self {
        Self { policy }
    }

    /// Create an orchestrator using [`DefaultPromotionPolicy`].
    pub fn with_default_policy() -> Self {
        Self::new(Box::new(DefaultPromotionPolicy::default()))
    }

    /// Store a new fact in the orchestrator's graph.
    pub fn store_fact(
        &mut self,
        _concept: &str,
        _content: &str,
        _confidence: f64,
        _source_id: &str,
    ) -> Result<String> {
        todo!()
    }

    /// Query facts by concept.
    pub fn query(&self, _concept: &str) -> Result<Vec<HiveFact>> {
        todo!()
    }

    /// Attempt to promote a fact for a specific agent.
    pub fn promote(&mut self, _fact_id: &str, _agent_id: &str) -> Result<bool> {
        todo!()
    }

    /// Return a reference to the active promotion policy.
    pub fn policy(&self) -> &dyn PromotionPolicy {
        &*self.policy
    }
}
