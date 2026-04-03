//! Memory search abstraction, ranking, and filtering.
//!
//! Port of Python `memory_retrieval.py` — provides FTS query normalization
//! and the `MemoryRetrieverStore` wrapper that bridges the memory backend
//! to the agent-core `MemoryRetriever` trait.

use std::collections::HashMap;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// FTS query normalisation
// ---------------------------------------------------------------------------

/// Pattern matching structured IDs like `INC-2024-001` that need quoting for
/// SQLite FTS5 (hyphens are otherwise parsed as query operators).
///
/// Uses a simple word-boundary match. The `normalize_fts_query` function
/// skips IDs that are already enclosed in double quotes via post-processing.
static STRUCTURED_ID_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b([A-Z]{2,5}-\d{4}-\d{2,5})\b").unwrap());

/// Normalize a text query for SQLite FTS5 by quoting structured identifiers.
///
/// Hyphenated IDs like `INC-2024-001` are wrapped in double quotes so the FTS
/// engine treats them as literal phrases rather than query syntax.
pub fn normalize_fts_query(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Replace only IDs that are not already double-quoted.
    let mut result = trimmed.to_string();
    for cap in STRUCTURED_ID_RE.captures_iter(trimmed) {
        let id = &cap[1];
        let quoted = format!("\"{id}\"");
        // Only replace bare IDs (not already inside quotes).
        let bare = id.to_string();
        if !result.contains(&quoted) {
            result = result.replacen(&bare, &quoted, 1);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Search result
// ---------------------------------------------------------------------------

/// A single memory search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub experience_id: String,
    pub context: String,
    pub outcome: String,
    pub confidence: f64,
    pub timestamp: String,
    pub tags: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Memory statistics
// ---------------------------------------------------------------------------

/// Storage statistics returned by the memory backend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryStats {
    pub total_experiences: usize,
    #[serde(default)]
    pub by_type: HashMap<String, usize>,
    pub storage_size_kb: f64,
}

// ---------------------------------------------------------------------------
// Store configuration
// ---------------------------------------------------------------------------

/// Configuration for constructing a `MemoryRetrieverStore`.
#[derive(Debug, Clone)]
pub struct RetrieverConfig {
    pub agent_name: String,
    pub storage_path: Option<String>,
}

impl RetrieverConfig {
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
            storage_path: None,
        }
    }

    pub fn with_storage_path(mut self, path: impl Into<String>) -> Self {
        self.storage_path = Some(path.into());
        self
    }
}

// ---------------------------------------------------------------------------
// MemoryRetrieverStore
// ---------------------------------------------------------------------------

/// In-memory implementation of a memory retriever for testing and lightweight use.
///
/// In production, the Python version delegates to `ExperienceStore` backed by
/// a Kuzu graph database. This Rust version stores facts in a `Vec` and
/// provides simple substring search — suitable for unit tests and embedding
/// in the agent-core crate without a database dependency.
#[derive(Debug)]
pub struct MemoryRetrieverStore {
    agent_name: String,
    facts: Vec<SearchResult>,
}

impl MemoryRetrieverStore {
    /// Create a new retriever.
    ///
    /// # Panics
    ///
    /// Panics if `config.agent_name` is empty.
    pub fn new(config: RetrieverConfig) -> Self {
        assert!(
            !config.agent_name.trim().is_empty(),
            "agent_name cannot be empty"
        );
        Self {
            agent_name: config.agent_name,
            facts: Vec::new(),
        }
    }

    /// Agent name.
    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    /// Search for facts matching `query` (substring match, case-insensitive).
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        min_confidence: f64,
    ) -> Vec<SearchResult> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let normalized = normalize_fts_query(query);
        let lower = normalized.to_lowercase();

        self.facts
            .iter()
            .filter(|f| {
                f.confidence >= min_confidence
                    && (f.context.to_lowercase().contains(&lower)
                        || f.outcome.to_lowercase().contains(&lower))
            })
            .take(limit)
            .cloned()
            .collect()
    }

    /// Store a learned fact.
    ///
    /// # Errors
    ///
    /// Returns `Err` if `context` or `fact` is empty, or `confidence` is
    /// outside `[0.0, 1.0]`.
    pub fn store_fact(
        &mut self,
        context: &str,
        fact: &str,
        confidence: f64,
        tags: &[String],
    ) -> Result<String, String> {
        if context.trim().is_empty() {
            return Err("context cannot be empty".into());
        }
        if fact.trim().is_empty() {
            return Err("fact cannot be empty".into());
        }
        if !(0.0..=1.0).contains(&confidence) {
            return Err("confidence must be between 0.0 and 1.0".into());
        }
        let id = format!("exp-{}", self.facts.len() + 1);
        self.facts.push(SearchResult {
            experience_id: id.clone(),
            context: context.trim().to_string(),
            outcome: fact.trim().to_string(),
            confidence,
            timestamp: chrono::Utc::now().to_rfc3339(),
            tags: tags.to_vec(),
            metadata: HashMap::new(),
        });
        Ok(id)
    }

    /// Retrieve all facts (up to `limit`), most recent first.
    pub fn get_all_facts(&self, limit: usize) -> Vec<SearchResult> {
        self.facts.iter().rev().take(limit).cloned().collect()
    }

    /// Storage statistics.
    pub fn get_statistics(&self) -> MemoryStats {
        MemoryStats {
            total_experiences: self.facts.len(),
            by_type: HashMap::new(),
            storage_size_kb: 0.0,
        }
    }

    /// Number of stored facts.
    pub fn len(&self) -> usize {
        self.facts.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_plain() {
        assert_eq!(normalize_fts_query("hello world"), "hello world");
    }

    #[test]
    fn normalize_structured_id() {
        let q = normalize_fts_query("incident INC-2024-001 details");
        assert!(q.contains("\"INC-2024-001\""), "got: {q}");
    }

    #[test]
    fn normalize_already_quoted() {
        let q = normalize_fts_query(r#"incident "INC-2024-001" details"#);
        // Should not double-quote
        assert_eq!(
            q.matches("\"INC-2024-001\"").count(),
            1,
            "got: {q}"
        );
    }

    #[test]
    fn normalize_empty() {
        assert_eq!(normalize_fts_query("  "), "");
    }

    #[test]
    fn store_and_search() {
        let cfg = RetrieverConfig::new("test-agent");
        let mut store = MemoryRetrieverStore::new(cfg);

        let id = store
            .store_fact("Photosynthesis", "Plants convert light to energy", 0.9, &[])
            .unwrap();
        assert!(id.starts_with("exp-"));
        assert_eq!(store.len(), 1);

        let results = store.search("photosynthesis", 10, 0.0);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].outcome, "Plants convert light to energy");
    }

    #[test]
    fn store_validation() {
        let cfg = RetrieverConfig::new("test-agent");
        let mut store = MemoryRetrieverStore::new(cfg);

        assert!(store.store_fact("", "fact", 0.9, &[]).is_err());
        assert!(store.store_fact("ctx", "", 0.9, &[]).is_err());
        assert!(store.store_fact("ctx", "fact", 1.5, &[]).is_err());
        assert!(store.store_fact("ctx", "fact", -0.1, &[]).is_err());
    }

    #[test]
    fn search_empty_query() {
        let cfg = RetrieverConfig::new("test-agent");
        let store = MemoryRetrieverStore::new(cfg);
        assert!(store.search("", 10, 0.0).is_empty());
    }

    #[test]
    fn search_min_confidence() {
        let cfg = RetrieverConfig::new("test-agent");
        let mut store = MemoryRetrieverStore::new(cfg);
        store
            .store_fact("low", "low confidence fact", 0.3, &[])
            .unwrap();
        store
            .store_fact("high", "high confidence fact", 0.9, &[])
            .unwrap();

        let results = store.search("confidence", 10, 0.5);
        assert_eq!(results.len(), 1);
        assert!(results[0].confidence >= 0.5);
    }

    #[test]
    fn get_all_facts_order() {
        let cfg = RetrieverConfig::new("test-agent");
        let mut store = MemoryRetrieverStore::new(cfg);
        store.store_fact("First", "fact 1", 0.9, &[]).unwrap();
        store.store_fact("Second", "fact 2", 0.9, &[]).unwrap();

        let all = store.get_all_facts(10);
        // Most recent first
        assert_eq!(all[0].context, "Second");
        assert_eq!(all[1].context, "First");
    }

    #[test]
    fn statistics() {
        let cfg = RetrieverConfig::new("test-agent");
        let mut store = MemoryRetrieverStore::new(cfg);
        store.store_fact("ctx", "fact", 0.9, &[]).unwrap();
        let stats = store.get_statistics();
        assert_eq!(stats.total_experiences, 1);
    }

    #[test]
    #[should_panic(expected = "agent_name cannot be empty")]
    fn empty_agent_name_panics() {
        MemoryRetrieverStore::new(RetrieverConfig::new(""));
    }

    #[test]
    fn config_builder() {
        let cfg = RetrieverConfig::new("agent").with_storage_path("/data");
        assert_eq!(cfg.agent_name, "agent");
        assert_eq!(cfg.storage_path.as_deref(), Some("/data"));
    }
}
