use chrono::Utc;
use uuid::Uuid;

use crate::error::Result;
use crate::models::HiveFact;

/// In-memory knowledge graph storing [`HiveFact`]s.
pub struct HiveGraph {
    facts: Vec<HiveFact>,
}

impl HiveGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self { facts: Vec::new() }
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
        };
        self.facts.push(fact);
        Ok(id)
    }

    /// Query facts by concept with a minimum confidence threshold.
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
                f.concept.to_lowercase().contains(&concept_lower) && f.confidence >= min_confidence
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
}

impl Default for HiveGraph {
    fn default() -> Self {
        Self::new()
    }
}
