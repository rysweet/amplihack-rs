//! In-memory knowledge graph for the hive mind.

mod agents;
mod edges;
mod federation;
pub mod search;

use std::collections::HashMap;

use chrono::Utc;
use uuid::Uuid;

use crate::error::{HiveError, Result};
use crate::models::{GraphStats, HiveAgent, HiveEdge, HiveFact};

pub const CONFIDENCE_SCORE_BOOST: f64 = 0.3;
pub const GOSSIP_TAG_PREFIX: &str = "gossip:";
pub const BROADCAST_TAG_PREFIX: &str = "broadcast:";
pub const ESCALATION_TAG_PREFIX: &str = "escalation:";

/// In-memory knowledge graph storing facts, agents, and edges.
pub struct HiveGraph {
    pub(crate) hive_id: String,
    pub(crate) facts: Vec<HiveFact>,
    pub(crate) agents: HashMap<String, HiveAgent>,
    pub(crate) edges: Vec<HiveEdge>,
    pub(crate) parent_id: Option<String>,
    pub(crate) children_ids: Vec<String>,
}

impl HiveGraph {
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

    pub fn with_id(hive_id: impl Into<String>) -> Self {
        Self {
            hive_id: hive_id.into(),
            facts: Vec::new(),
            agents: HashMap::new(),
            edges: Vec::new(),
            parent_id: None,
            children_ids: Vec::new(),
        }
    }

    pub fn hive_id(&self) -> &str {
        &self.hive_id
    }

    pub fn store_fact(
        &mut self,
        concept: &str,
        content: &str,
        confidence: f64,
        source_id: &str,
        tags: Vec<String>,
    ) -> Result<String> {
        if !(0.0..=1.0).contains(&confidence) {
            return Err(HiveError::InvalidConfidence(confidence));
        }
        let id = Uuid::new_v4().to_string();
        self.facts.push(HiveFact {
            fact_id: id.clone(),
            concept: concept.to_string(),
            content: content.to_string(),
            confidence,
            source_id: source_id.to_string(),
            tags,
            created_at: Utc::now(),
            status: "promoted".to_string(),
            metadata: HashMap::new(),
        });
        Ok(id)
    }

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

    pub fn get_fact(&self, fact_id: &str) -> Result<Option<HiveFact>> {
        Ok(self.facts.iter().find(|f| f.fact_id == fact_id).cloned())
    }

    pub fn remove_fact(&mut self, fact_id: &str) -> Result<bool> {
        let len_before = self.facts.len();
        self.facts.retain(|f| f.fact_id != fact_id);
        Ok(self.facts.len() < len_before)
    }

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

    pub fn facts_by_tag(&self, tag: &str) -> Result<Vec<HiveFact>> {
        Ok(self
            .facts
            .iter()
            .filter(|f| f.tags.iter().any(|t| t == tag))
            .cloned()
            .collect())
    }

    pub fn fact_count(&self) -> usize {
        self.facts.len()
    }

    pub fn all_facts(&self) -> Vec<HiveFact> {
        self.facts.clone()
    }

    pub fn set_fact_confidence(&mut self, fact_id: &str, confidence: f64) -> bool {
        if let Some(fact) = self.facts.iter_mut().find(|f| f.fact_id == fact_id) {
            fact.confidence = confidence.clamp(0.0, 1.0);
            true
        } else {
            false
        }
    }

    pub fn get_stats(&self) -> GraphStats {
        GraphStats {
            fact_count: self.facts.len(),
            agent_count: self.agents.len(),
            edge_count: self.edges.len(),
            retracted_count: self
                .facts
                .iter()
                .filter(|f| f.status == "retracted")
                .count(),
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
