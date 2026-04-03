//! Core types for the retrieval subsystem.
//!
//! Ported from Python `retrieval_strategies.py` — provides the `Fact` struct,
//! `MemorySearch` trait, intent enum, and aggregation result types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Fact
// ---------------------------------------------------------------------------

/// A single memory fact (the dict[str, Any] from Python).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub experience_id: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Fact {
    pub fn new(context: impl Into<String>, outcome: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            outcome: outcome.into(),
            confidence: 0.8,
            timestamp: String::new(),
            experience_id: String::new(),
            tags: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Deduplication key: experience_id if present, else `"{context}::{outcome}"`.
    pub fn dedup_key(&self) -> String {
        if !self.experience_id.is_empty() {
            self.experience_id.clone()
        } else {
            format!("{}::{}", self.context, self.outcome)
        }
    }

    /// Temporal sort key: `(temporal_index, timestamp)`.
    pub fn temporal_sort_key(&self) -> (i64, String) {
        let t_idx = self
            .metadata
            .get("temporal_index")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        (t_idx, self.timestamp.clone())
    }

    /// Build a summary fact for a group.
    pub fn summary(group_key: &str, count: usize, combined: &str, level: &str) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("is_summary".into(), serde_json::Value::Bool(true));
        metadata.insert(
            "source_count".into(),
            serde_json::Value::Number(serde_json::Number::from(count)),
        );
        Self {
            context: format!("SUMMARY ({group_key})"),
            outcome: format!("[Summary of {count} facts about {group_key}]: {combined}"),
            confidence: 0.7,
            timestamp: String::new(),
            experience_id: String::new(),
            tags: vec!["summary".into(), level.into()],
            metadata,
        }
    }
}

// ---------------------------------------------------------------------------
// Aggregation result
// ---------------------------------------------------------------------------

/// Result returned by graph aggregation queries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregationResult {
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub contents: Vec<String>,
    #[serde(default)]
    pub count: Option<usize>,
    #[serde(default)]
    pub item_counts: HashMap<String, usize>,
}

// ---------------------------------------------------------------------------
// Memory statistics
// ---------------------------------------------------------------------------

/// Statistics extracted from a memory backend.
#[derive(Debug, Clone, Default)]
pub struct MemoryStatistics {
    pub total_experiences: Option<usize>,
    pub total: Option<usize>,
    pub total_facts: Option<usize>,
    pub semantic: Option<usize>,
    pub semantic_nodes: Option<usize>,
    pub episodic_nodes: Option<usize>,
}

impl MemoryStatistics {
    /// Extract the best estimate of total local fact count.
    pub fn estimated_total(&self) -> Option<usize> {
        if let Some(v) = self.total_experiences {
            return Some(v);
        }
        if let Some(v) = self.total {
            return Some(v);
        }
        if let Some(v) = self.total_facts {
            return Some(v);
        }
        if let Some(v) = self.semantic {
            return Some(v);
        }
        match (self.semantic_nodes, self.episodic_nodes) {
            (Some(s), Some(e)) => Some(s + e),
            (Some(s), None) => Some(s),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Memory search trait
// ---------------------------------------------------------------------------

/// Trait abstracting memory backends for retrieval strategies.
pub trait MemorySearch {
    /// Return up to `limit` facts, optionally filtered by `query`.
    fn get_all_facts(&self, limit: usize, query: &str) -> Vec<Fact>;

    /// Text search returning up to `limit` results.
    fn search(&self, query: &str, limit: usize) -> Vec<Fact>;

    /// Local-only text search (for distributed systems).
    fn search_local(&self, query: &str, limit: usize) -> Vec<Fact> {
        self.search(query, limit)
    }

    /// Concept-keyword search.
    fn search_by_concept(&self, keywords: &[String], limit: usize) -> Vec<Fact> {
        if let Some(kw) = keywords.first() {
            self.search(kw, limit)
        } else {
            Vec::new()
        }
    }

    /// Local-only concept-keyword search.
    fn search_by_concept_local(&self, keywords: &[String], limit: usize) -> Vec<Fact> {
        self.search_by_concept(keywords, limit)
    }

    /// Entity-centric retrieval.
    fn retrieve_by_entity(&self, entity_name: &str, limit: usize) -> Vec<Fact> {
        self.search(entity_name, limit)
    }

    /// Local-only entity-centric retrieval.
    fn retrieve_by_entity_local(&self, entity_name: &str, limit: usize) -> Vec<Fact> {
        self.retrieve_by_entity(entity_name, limit)
    }

    /// Backend statistics for size estimation.
    fn get_statistics(&self) -> Option<MemoryStatistics> {
        None
    }

    /// Execute a graph aggregation query.
    fn execute_aggregation(
        &self,
        _query_name: &str,
        _entity_filter: Option<&str>,
    ) -> AggregationResult {
        AggregationResult::default()
    }

    /// Whether this backend supports hierarchical (graph) features.
    fn supports_hierarchical(&self) -> bool {
        false
    }

    /// Whether this backend has distributed (local) search.
    fn supports_local_search(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Intent classification
// ---------------------------------------------------------------------------

/// Classified intent of a user question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentKind {
    SimpleRecall,
    IncrementalUpdate,
    ContradictionResolution,
    MultiSourceSynthesis,
    CausalCounterfactual,
    MetaMemory,
    Temporal,
    Mathematical,
}

impl IntentKind {
    /// Whether this intent uses simple (single-pass) retrieval.
    pub fn is_simple(&self) -> bool {
        matches!(
            self,
            Self::SimpleRecall
                | Self::IncrementalUpdate
                | Self::ContradictionResolution
                | Self::MultiSourceSynthesis
                | Self::CausalCounterfactual
        )
    }

    /// Whether this intent routes to graph aggregation.
    pub fn is_aggregation(&self) -> bool {
        matches!(self, Self::MetaMemory)
    }
}
