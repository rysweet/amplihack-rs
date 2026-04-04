//! Flat retriever adapter wrapping `HierarchicalMemoryLocal`.
//!
//! Port of Python `flat_retriever_adapter.py` — adapter pattern providing a
//! `MemoryRetriever`-compatible interface over [`HierarchicalMemoryLocal`],
//! so callers that expect `store_fact/search/get_all_facts` can use the
//! hierarchical memory without changes.

use std::collections::HashMap;

use crate::error::{AgentError, Result};
use crate::hierarchical_memory_local::HierarchicalMemoryLocal;
use crate::hierarchical_memory_types::{KnowledgeNode, MemoryCategory, StoreKnowledgeParams};
use crate::memory_retrieval::SearchResult;

// ── FlatRetrieverAdapter ─────────────────────────────────────────────────

/// Backward-compatible adapter over [`HierarchicalMemoryLocal`].
///
/// Maps:
/// - `store_fact` → `store_knowledge(category=Semantic)`
/// - `search` → `retrieve_subgraph` + flatten
/// - `get_all_facts` → `get_all_knowledge` + flatten
pub struct FlatRetrieverAdapter {
    agent_name: String,
    memory: HierarchicalMemoryLocal,
}

impl FlatRetrieverAdapter {
    /// Create a new adapter for the given agent.
    pub fn new(agent_name: impl Into<String>) -> Self {
        let name = agent_name.into();
        Self {
            memory: HierarchicalMemoryLocal::new(&name),
            agent_name: name,
        }
    }

    /// Agent name.
    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    /// Store a fact as semantic knowledge.
    ///
    /// # Errors
    ///
    /// Returns an error if `context` or `fact` is empty, or `confidence`
    /// is outside `[0.0, 1.0]`.
    pub fn store_fact(
        &mut self,
        context: &str,
        fact: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
        temporal_metadata: Option<&HashMap<String, serde_json::Value>>,
    ) -> Result<String> {
        if context.trim().is_empty() {
            return Err(AgentError::MemoryError("context cannot be empty".into()));
        }
        if fact.trim().is_empty() {
            return Err(AgentError::MemoryError("fact cannot be empty".into()));
        }
        if !(0.0..=1.0).contains(&confidence) {
            return Err(AgentError::MemoryError(
                "confidence must be between 0.0 and 1.0".into(),
            ));
        }

        Ok(self.memory.store_knowledge(StoreKnowledgeParams {
            content: fact.trim(),
            concept: context.trim(),
            confidence,
            category: MemoryCategory::Semantic,
            source_id,
            tags,
            temporal_metadata,
        }))
    }

    /// Search memory and return flat list of [`SearchResult`].
    pub fn search(&self, query: &str, limit: usize, min_confidence: f64) -> Vec<SearchResult> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let subgraph = self.memory.retrieve_subgraph(query.trim(), limit);

        subgraph
            .nodes
            .iter()
            .filter(|n| n.confidence >= min_confidence)
            .take(limit)
            .map(node_to_search_result)
            .collect()
    }

    /// Retrieve all facts without keyword filtering.
    pub fn get_all_facts(&self, limit: usize) -> Vec<SearchResult> {
        self.memory
            .get_all_knowledge(limit)
            .iter()
            .map(node_to_search_result)
            .collect()
    }

    /// Get memory statistics.
    pub fn get_statistics(&self) -> HashMap<String, serde_json::Value> {
        self.memory.get_statistics()
    }

    /// Retrieve all facts about a specific entity.
    pub fn retrieve_by_entity(&self, entity_name: &str, limit: usize) -> Vec<SearchResult> {
        self.memory
            .retrieve_by_entity(entity_name, limit)
            .iter()
            .map(node_to_search_result)
            .collect()
    }

    /// Search for facts by concept/content keyword matching.
    pub fn search_by_concept(&self, keywords: &[String], limit: usize) -> Vec<SearchResult> {
        self.memory
            .search_by_concept(keywords, limit)
            .iter()
            .map(node_to_search_result)
            .collect()
    }

    /// Execute an aggregation query.
    pub fn execute_aggregation(
        &self,
        query_type: &str,
        entity_filter: &str,
    ) -> HashMap<String, serde_json::Value> {
        self.memory.execute_aggregation(query_type, entity_filter)
    }

    /// Store an episode (raw source content).
    pub fn store_episode(&mut self, content: &str, source_label: &str) -> String {
        self.memory.store_episode(content, source_label)
    }

    /// Flush underlying memory cache.
    pub fn flush_memory(&self) {
        self.memory.flush_memory();
    }

    /// Close underlying memory.
    pub fn close(&self) {
        self.memory.close();
    }
}

/// Convert a [`KnowledgeNode`] to a [`SearchResult`].
fn node_to_search_result(node: &KnowledgeNode) -> SearchResult {
    SearchResult {
        experience_id: node.node_id.clone(),
        context: node.concept.clone(),
        outcome: node.content.clone(),
        confidence: node.confidence,
        timestamp: node.created_at.clone(),
        tags: node.tags.clone(),
        metadata: node.metadata.clone(),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_search() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        let id = adapter
            .store_fact(
                "Biology",
                "Cells are the basic unit of life",
                0.9,
                &[],
                "",
                None,
            )
            .unwrap();
        assert!(!id.is_empty());

        let results = adapter.search("cells", 10, 0.0);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].context, "Biology");
        assert_eq!(results[0].outcome, "Cells are the basic unit of life");
    }

    #[test]
    fn store_validation_empty_context() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        let err = adapter.store_fact("", "fact", 0.9, &[], "", None);
        assert!(err.is_err());
    }

    #[test]
    fn store_validation_empty_fact() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        let err = adapter.store_fact("ctx", "", 0.9, &[], "", None);
        assert!(err.is_err());
    }

    #[test]
    fn store_validation_bad_confidence() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        assert!(
            adapter
                .store_fact("ctx", "fact", 1.5, &[], "", None)
                .is_err()
        );
        assert!(
            adapter
                .store_fact("ctx", "fact", -0.1, &[], "", None)
                .is_err()
        );
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let adapter = FlatRetrieverAdapter::new("test-agent");
        assert!(adapter.search("", 10, 0.0).is_empty());
    }

    #[test]
    fn search_min_confidence_filters() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        adapter
            .store_fact("Topic", "low confidence fact", 0.3, &[], "", None)
            .unwrap();
        adapter
            .store_fact("Topic", "high confidence fact", 0.9, &[], "", None)
            .unwrap();

        let results = adapter.search("confidence fact", 10, 0.5);
        assert!(results.iter().all(|r| r.confidence >= 0.5));
    }

    #[test]
    fn get_all_facts_returns_all() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        adapter
            .store_fact("A", "fact one", 0.9, &[], "", None)
            .unwrap();
        adapter
            .store_fact("B", "fact two", 0.8, &[], "", None)
            .unwrap();

        let all = adapter.get_all_facts(50);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn retrieve_by_entity_works() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        adapter
            .store_fact("Mars", "Mars has polar ice caps", 0.9, &[], "", None)
            .unwrap();
        adapter
            .store_fact("Venus", "Venus is very hot", 0.8, &[], "", None)
            .unwrap();

        let results = adapter.retrieve_by_entity("mars", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].context, "Mars");
    }

    #[test]
    fn search_by_concept_works() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        adapter
            .store_fact("Genetics", "DNA stores information", 0.9, &[], "", None)
            .unwrap();
        adapter
            .store_fact("Physics", "Light is a wave", 0.8, &[], "", None)
            .unwrap();

        let results = adapter.search_by_concept(&["genetics".into()], 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn execute_aggregation_count() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        adapter
            .store_fact("A", "fact1", 0.9, &[], "", None)
            .unwrap();
        adapter
            .store_fact("B", "fact2", 0.8, &[], "", None)
            .unwrap();

        let result = adapter.execute_aggregation("count_entities", "");
        assert_eq!(result["count"], serde_json::json!(2));
    }

    #[test]
    fn store_episode_works() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        let eid = adapter.store_episode("raw content", "source");
        assert!(eid.starts_with("ep-"));
    }

    #[test]
    fn statistics_after_operations() {
        let mut adapter = FlatRetrieverAdapter::new("test-agent");
        adapter.store_fact("A", "fact", 0.9, &[], "", None).unwrap();
        let stats = adapter.get_statistics();
        assert_eq!(stats["total_nodes"], serde_json::json!(1));
    }
}
