pub mod agents;
pub mod edges;
pub mod federation;
pub mod search;

use std::collections::HashMap;

use chrono::Utc;
use uuid::Uuid;

use crate::error::Result;
use crate::models::{GraphStats, HiveAgent, HiveEdge, HiveFact};

pub const CONFIDENCE_SCORE_BOOST: f64 = 0.3;
pub const GOSSIP_TAG_PREFIX: &str = "gossip:";
pub const BROADCAST_TAG_PREFIX: &str = "broadcast:";
pub const ESCALATION_TAG_PREFIX: &str = "escalation:";

/// In-memory knowledge graph storing [`HiveFact`]s, agents, and edges.
pub struct HiveGraph {
    pub(crate) hive_id: String,
    pub(crate) facts: Vec<HiveFact>,
    pub(crate) agents: HashMap<String, HiveAgent>,
    pub(crate) edges: Vec<HiveEdge>,
    pub(crate) parent_id: Option<String>,
    pub(crate) children_ids: Vec<String>,
}

impl HiveGraph {
    /// Create an empty graph with a random ID.
    pub fn new() -> Self {
        Self {
            hive_id: Uuid::new_v4().to_string(),
            facts: Vec::new(),
            agents: HashMap::new(),
            edges: Vec::new(),
            parent_id: None,
            children_ids: Vec::new(),
        }
    }

    /// Create an empty graph with the given ID.
    pub fn with_id(id: impl Into<String>) -> Self {
        Self {
            hive_id: id.into(),
            facts: Vec::new(),
            agents: HashMap::new(),
            edges: Vec::new(),
            parent_id: None,
            children_ids: Vec::new(),
        }
    }

    /// Return the hive ID.
    pub fn hive_id(&self) -> &str {
        &self.hive_id
    }

    /// Store a new fact and return its generated ID.
    pub fn store_fact(
        &mut self,
        concept: &str,
        content: &str,
        confidence: f64,
        source_id: &str,
        tags: Vec<String>,
    ) -> Result<String> {
        if !(0.0..=1.0).contains(&confidence) {
            return Err(crate::error::HiveError::InvalidConfidence(confidence));
        }
        let id = Uuid::new_v4().to_string();
        let fact = HiveFact {
            fact_id: id.clone(),
            concept: concept.to_string(),
            content: content.to_string(),
            confidence,
            source_id: source_id.to_string(),
            tags,
            created_at: Utc::now(),
            status: "promoted".to_string(),
            metadata: HashMap::new(),
        };
        self.facts.push(fact);
        Ok(id)
    }

    /// Query facts by concept with a minimum confidence threshold.
    /// Excludes retracted facts.
    pub fn query_facts(
        &self,
        concept: &str,
        min_confidence: f64,
        limit: usize,
    ) -> Result<Vec<HiveFact>> {
        let concept_lower = concept.to_lowercase();
        let mut results: Vec<HiveFact> = self
            .facts
            .iter()
            .filter(|f| {
                f.status != "retracted"
                    && f.concept.to_lowercase().contains(&concept_lower)
                    && f.confidence >= min_confidence
            })
            .cloned()
            .collect();
        results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);
        Ok(results)
    }

    /// Retrieve a single fact by ID.
    pub fn get_fact(&self, fact_id: &str) -> Result<Option<HiveFact>> {
        Ok(self.facts.iter().find(|f| f.fact_id == fact_id).cloned())
    }

    /// Remove a fact by ID, returning whether it existed.
    pub fn remove_fact(&mut self, fact_id: &str) -> Result<bool> {
        let len_before = self.facts.len();
        self.facts.retain(|f| f.fact_id != fact_id);
        Ok(self.facts.len() < len_before)
    }

    /// Retract a fact by ID, setting its status and recording the reason.
    /// Returns `true` if the fact was found and retracted.
    pub fn retract_fact(&mut self, fact_id: &str, reason: &str) -> bool {
        if let Some(fact) = self.facts.iter_mut().find(|f| f.fact_id == fact_id) {
            fact.status = "retracted".to_string();
            fact.metadata
                .insert("retraction_reason".to_string(), reason.to_string());
            true
        } else {
            false
        }
    }

    /// Return all facts tagged with the given tag.
    pub fn facts_by_tag(&self, tag: &str) -> Result<Vec<HiveFact>> {
        Ok(self
            .facts
            .iter()
            .filter(|f| f.tags.iter().any(|t| t == tag))
            .cloned()
            .collect())
    }

    /// Return the total number of stored facts.
    pub fn fact_count(&self) -> usize {
        self.facts.len()
    }

    /// Return all facts.
    pub fn all_facts(&self) -> &[HiveFact] {
        &self.facts
    }

    /// Update a fact's confidence by ID, clamping to \[0.0, 1.0\].
    ///
    /// Returns `true` if the fact was found and updated.
    pub fn set_fact_confidence(&mut self, fact_id: &str, confidence: f64) -> bool {
        if let Some(fact) = self.facts.iter_mut().find(|f| f.fact_id == fact_id) {
            fact.confidence = confidence.clamp(0.0, 1.0);
            true
        } else {
            false
        }
    }

    /// Return summary statistics for this graph.
    pub fn get_stats(&self) -> GraphStats {
        GraphStats {
            fact_count: self.facts.len(),
            agent_count: self.agents.len(),
            edge_count: self.edges.len(),
            retracted_count: self.facts.iter().filter(|f| f.status == "retracted").count(),
            active_agent_count: self
                .agents
                .values()
                .filter(|a| a.status == "active")
                .count(),
        }
    }
}

impl Default for HiveGraph {
    fn default() -> Self {
        Self::new()
    }
}
