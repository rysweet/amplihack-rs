//! Memory coordinator — central orchestration with quality control.
//!
//! Matches Python `amplihack/memory/coordinator.py`:
//! - Trivial content filter
//! - Duplicate detection via content fingerprinting
//! - Importance scoring and relevance ranking
//! - Token budget enforcement (~4 chars per token)
//! - Storage and retrieval pipelines

use crate::config::MemoryConfig;
use crate::models::{MemoryEntry, MemoryQuery, MemoryType, StorageRequest};
use crate::quality::{is_trivial, matches_query, relevance_score, score_importance};
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

const CHARS_PER_TOKEN: usize = 4;

/// Statistics for the coordinator.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CoordinatorStats {
    pub total_stored: u64,
    pub total_retrieved: u64,
    pub total_rejected: u64,
    pub duplicate_count: u64,
    pub trivial_count: u64,
    pub entries_by_type: HashMap<String, u64>,
}

/// Central memory coordinator with quality control pipeline.
pub struct MemoryCoordinator {
    config: MemoryConfig,
    entries: Vec<MemoryEntry>,
    fingerprints: HashSet<u64>,
    stats: CoordinatorStats,
}

impl MemoryCoordinator {
    pub fn new(config: MemoryConfig) -> Self {
        Self {
            config,
            entries: Vec::new(),
            fingerprints: HashSet::new(),
            stats: CoordinatorStats::default(),
        }
    }

    /// Store a memory entry through the quality pipeline.
    ///
    /// Pipeline: trivial filter → duplicate check → importance scoring → store
    pub fn store(&mut self, request: StorageRequest) -> Option<String> {
        // Stage 1: Trivial content filter
        if self.config.trivial_content_filter
            && is_trivial(&request.content, self.config.min_content_length)
        {
            debug!(
                content_len = request.content.len(),
                "Rejected: trivial content"
            );
            self.stats.trivial_count += 1;
            self.stats.total_rejected += 1;
            return None;
        }

        // Stage 2: Duplicate detection
        let mut entry = MemoryEntry::new(
            &request.session_id,
            &request.agent_id,
            request.memory_type,
            &request.content,
        );
        entry.metadata = request.metadata;

        if self.config.duplicate_detection {
            let fp = entry.content_fingerprint();
            if self.fingerprints.contains(&fp) {
                debug!("Rejected: duplicate content");
                self.stats.duplicate_count += 1;
                self.stats.total_rejected += 1;
                return None;
            }
            self.fingerprints.insert(fp);
        }

        // Stage 3: Importance scoring
        entry.importance = request
            .importance
            .unwrap_or_else(|| score_importance(&entry.content, entry.memory_type));

        let id = entry.id.clone();
        info!(id = %id, mem_type = entry.memory_type.as_str(), "Stored memory");
        *self
            .stats
            .entries_by_type
            .entry(entry.memory_type.as_str().to_string())
            .or_insert(0) += 1;
        self.stats.total_stored += 1;
        self.entries.push(entry);
        Some(id)
    }

    /// Retrieve memories matching the query within the token budget.
    ///
    /// Pipeline: filter → rank → budget enforcement → deduplicate
    pub fn retrieve(&mut self, query: &MemoryQuery) -> Vec<MemoryEntry> {
        self.stats.total_retrieved += 1;

        let mut candidates: Vec<&MemoryEntry> = self
            .entries
            .iter()
            .filter(|e| matches_query(e, query))
            .collect();

        // Rank by relevance
        let query_words: HashSet<&str> = query.query_text.split_whitespace().collect();
        candidates.sort_by(|a, b| {
            let score_a = relevance_score(a, &query_words);
            let score_b = relevance_score(b, &query_words);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Token budget enforcement
        let budget_chars = query.token_budget * CHARS_PER_TOKEN;
        let mut used_chars = 0;
        let mut results = Vec::new();
        let limit = if query.limit > 0 { query.limit } else { 20 };

        for entry in candidates {
            if results.len() >= limit {
                break;
            }
            let entry_chars = entry.content.len() + entry.title.len();
            if used_chars + entry_chars > budget_chars && !results.is_empty() {
                break;
            }
            used_chars += entry_chars;
            results.push(entry.clone());
        }

        results
    }

    /// Clear working memory for a session.
    pub fn clear_working_memory(&mut self, session_id: &str) {
        self.entries
            .retain(|e| !(e.session_id == session_id && e.memory_type == MemoryType::Working));
        info!(session_id, "Cleared working memory");
    }

    /// Clear all memory for a session.
    pub fn clear_session(&mut self, session_id: &str) {
        let before = self.entries.len();
        self.entries.retain(|e| e.session_id != session_id);
        let removed = before - self.entries.len();
        info!(session_id, removed, "Cleared session memory");
    }

    /// Mark a task complete, clearing associated working memory.
    pub fn mark_task_complete(&mut self, session_id: &str) {
        self.clear_working_memory(session_id);
    }

    /// Get coordinator statistics.
    pub fn statistics(&self) -> &CoordinatorStats {
        &self.stats
    }

    /// Total entries stored.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_coordinator() -> MemoryCoordinator {
        MemoryCoordinator::new(MemoryConfig::for_testing())
    }

    fn store_req(content: &str) -> StorageRequest {
        StorageRequest::new(content, MemoryType::Semantic, "test-session")
    }

    #[test]
    fn stores_valid_content() {
        let mut coord = test_coordinator();
        let id = coord.store(store_req("The sky is blue and vast"));
        assert!(id.is_some());
        assert_eq!(coord.entry_count(), 1);
    }

    #[test]
    fn rejects_trivial_content() {
        let mut coord = MemoryCoordinator::new(MemoryConfig {
            trivial_content_filter: true,
            ..MemoryConfig::for_testing()
        });
        assert!(coord.store(store_req("hi")).is_none());
        assert!(coord.store(store_req("ok")).is_none());
        assert!(coord.store(store_req("short")).is_none());
        assert_eq!(coord.statistics().trivial_count, 3);
    }

    #[test]
    fn rejects_duplicates() {
        let mut coord = MemoryCoordinator::new(MemoryConfig {
            duplicate_detection: true,
            ..MemoryConfig::for_testing()
        });
        let content = "This is a unique and substantive memory entry";
        assert!(coord.store(store_req(content)).is_some());
        assert!(coord.store(store_req(content)).is_none());
        assert_eq!(coord.statistics().duplicate_count, 1);
    }

    #[test]
    fn retrieves_by_query_text() {
        let mut coord = test_coordinator();
        coord.store(store_req("The sky is blue and vast"));
        coord.store(store_req("Grass is green and lush"));
        let query = MemoryQuery::new("sky blue");
        let results = coord.retrieve(&query);
        assert!(!results.is_empty());
        assert!(results[0].content.contains("sky"));
    }

    #[test]
    fn retrieves_by_session() {
        let mut coord = test_coordinator();
        coord.store(store_req("Memory for test session"));
        let mut req = store_req("Memory for other session");
        req.session_id = "other".into();
        coord.store(req);
        let query = MemoryQuery::new("memory").with_session("test-session");
        let results = coord.retrieve(&query);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn retrieves_by_type_filter() {
        let mut coord = test_coordinator();
        coord.store(store_req("Semantic memory content here"));
        coord.store(StorageRequest::new(
            "Working memory content here",
            MemoryType::Working,
            "test-session",
        ));
        let query = MemoryQuery::new("memory").with_types(vec![MemoryType::Working]);
        let results = coord.retrieve(&query);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_type, MemoryType::Working);
    }

    #[test]
    fn token_budget_enforcement() {
        let mut coord = test_coordinator();
        for i in 0..100 {
            coord.store(store_req(&format!(
                "Entry number {i} with sufficient padding text here"
            )));
        }
        let query = MemoryQuery::new("entry").with_budget(10);
        let results = coord.retrieve(&query);
        let total_chars: usize = results.iter().map(|e| e.content.len()).sum();
        assert!(total_chars <= 10 * CHARS_PER_TOKEN + 100);
    }

    #[test]
    fn clear_working_memory() {
        let mut coord = test_coordinator();
        coord.store(store_req("Permanent semantic memory here"));
        coord.store(StorageRequest::new(
            "Temporary working memory here",
            MemoryType::Working,
            "s1",
        ));
        coord.clear_working_memory("s1");
        assert_eq!(coord.entry_count(), 1);
    }

    #[test]
    fn clear_session() {
        let mut coord = test_coordinator();
        coord.store(store_req("Session memory content here"));
        coord.clear_session("test-session");
        assert_eq!(coord.entry_count(), 0);
    }

    #[test]
    fn statistics_tracking() {
        let mut coord = MemoryCoordinator::new(MemoryConfig {
            trivial_content_filter: true,
            duplicate_detection: true,
            ..MemoryConfig::for_testing()
        });
        coord.store(store_req("Valid memory content here"));
        coord.store(store_req("hi")); // trivial
        coord.store(store_req("Valid memory content here")); // duplicate
        let stats = coord.statistics();
        assert_eq!(stats.total_stored, 1);
        assert_eq!(stats.total_rejected, 2);
    }
}
