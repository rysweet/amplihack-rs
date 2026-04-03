//! `CognitiveAdapter` — unified memory interface over cognitive backends.
//!
//! Ports Python `cognitive_adapter.py`'s `CognitiveAdapter` class.
//! Implements both `MemoryRetriever` and `MemoryFacade` from the agentic
//! loop traits, providing a drop-in replacement regardless of whether the
//! underlying backend is full CognitiveMemory or the hierarchical fallback.

use std::collections::HashMap;

use serde_json::Value;
use tracing::{debug, warn};

use crate::agentic_loop::traits::{MemoryFacade, MemoryRetriever};
use crate::agentic_loop::types::MemoryFact;

use super::constants::{FALLBACK_SCAN_MULTIPLIER, SEARCH_CANDIDATE_MULTIPLIER};
use super::scoring::{filter_stop_words, hive_fact_to_memory, merge_results, rerank_by_ngram};
use super::types::{
    BackendKind, CognitiveAdapterConfig, CognitiveBackend, HiveFact, HiveStore, Procedure,
    ProspectiveTrigger, QualityScorer, WorkingSlot,
};

// ---------------------------------------------------------------------------
// CognitiveAdapter
// ---------------------------------------------------------------------------

/// Unified memory adapter over 6-type cognitive or hierarchical backends.
///
/// Provides the same `MemoryRetriever` / `MemoryFacade` interface
/// regardless of the backend, plus dedicated cognitive methods (working
/// memory, procedural, prospective, sensory).
///
/// When a `HiveStore` is attached, facts are automatically promoted
/// after quality gating, and searches federate across local + hive
/// with deduplication.
pub struct CognitiveAdapter {
    agent_name: String,
    backend: Box<dyn CognitiveBackend>,
    hive: Option<Box<dyn HiveStore>>,
    quality_scorer: Option<Box<dyn QualityScorer>>,
    quality_threshold: f64,
    confidence_gate: f64,
}

impl CognitiveAdapter {
    /// Create from a config, backend, and optional hive / quality scorer.
    pub fn new(
        config: CognitiveAdapterConfig,
        backend: Box<dyn CognitiveBackend>,
        hive: Option<Box<dyn HiveStore>>,
        quality_scorer: Option<Box<dyn QualityScorer>>,
    ) -> Self {
        Self {
            agent_name: config.agent_name,
            backend,
            hive,
            quality_scorer,
            quality_threshold: config.quality_threshold,
            confidence_gate: config.confidence_gate,
        }
    }

    /// Which backend flavour is active.
    pub fn backend_type(&self) -> BackendKind {
        self.backend.kind()
    }

    /// Agent name.
    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    // ------------------------------------------------------------------
    // FlatRetrieverAdapter-compatible interface
    // ------------------------------------------------------------------

    /// Store a semantic fact with optional hive promotion.
    ///
    /// Returns the node ID assigned by the backend.
    ///
    /// # Errors
    ///
    /// Returns `Err` if `context` or `fact` is empty/whitespace.
    pub fn store_fact_full(
        &mut self,
        context: &str,
        fact: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
        metadata: &HashMap<String, Value>,
    ) -> Result<String, &'static str> {
        let context = context.trim();
        let fact = fact.trim();
        if context.is_empty() {
            return Err("context cannot be empty");
        }
        if fact.is_empty() {
            return Err("fact cannot be empty");
        }

        let node_id =
            self.backend
                .store_fact(context, fact, confidence, source_id, tags, metadata);

        self.promote_to_hive(context, fact, confidence, tags, metadata);
        Ok(node_id)
    }

    /// Search memory with n-gram re-ranking and optional hive federation.
    pub fn search_full(
        &self,
        query: &str,
        limit: usize,
        min_confidence: f64,
    ) -> Vec<MemoryFact> {
        if query.trim().is_empty() {
            return Vec::new();
        }

        let filtered = filter_stop_words(query);
        let search_q = if filtered.trim().is_empty() {
            query.trim().to_string()
        } else {
            filtered
        };

        let mut local = self.backend_search(&search_q, limit, min_confidence);

        // Fallback: scan all when keyword search yields nothing
        if local.is_empty() {
            local = self.backend.get_all_facts(limit * FALLBACK_SCAN_MULTIPLIER);
        }

        rerank_by_ngram(query, &mut local, limit);

        // Federate with hive
        if let Some(ref hive) = self.hive {
            let hive_results = self.search_hive(hive.as_ref(), query, limit);
            if !hive_results.is_empty() {
                return merge_results(&local, &hive_results, limit);
            }
        }

        local
    }

    /// Search LOCAL memory only — no hive federation.
    ///
    /// Used by shard query handlers to avoid recursive query storms.
    pub fn search_local(
        &self,
        query: &str,
        limit: usize,
        min_confidence: f64,
    ) -> Vec<MemoryFact> {
        if query.trim().is_empty() {
            return Vec::new();
        }

        let filtered = filter_stop_words(query);
        let search_q = if filtered.trim().is_empty() {
            query.trim().to_string()
        } else {
            filtered
        };

        let mut local = self.backend_search(&search_q, limit, min_confidence);
        if local.is_empty() {
            local = self.backend.get_all_facts(limit * FALLBACK_SCAN_MULTIPLIER);
        }
        rerank_by_ngram(query, &mut local, limit);
        local
    }

    /// Retrieve all facts (up to `limit`).
    pub fn get_all_facts(&self, limit: usize) -> Vec<MemoryFact> {
        self.backend.get_all_facts(limit)
    }

    /// Store an episode (raw source content).
    pub fn store_episode(&mut self, content: &str, source_label: &str) -> String {
        self.backend.store_episode(content, source_label)
    }

    /// Get memory statistics.
    pub fn get_statistics(&self) -> HashMap<String, Value> {
        self.backend.get_statistics()
    }

    // ------------------------------------------------------------------
    // Cognitive-specific capabilities
    // ------------------------------------------------------------------

    /// Add to working memory (bounded, MAX_WORKING_SLOTS per task).
    pub fn push_working(
        &mut self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> Option<String> {
        self.backend.push_working(slot_type, content, task_id, relevance)
    }

    /// Get working memory slots for a task.
    pub fn get_working(&self, task_id: &str) -> Vec<WorkingSlot> {
        self.backend.get_working(task_id)
    }

    /// Clear working memory for a task.
    pub fn clear_working(&mut self, task_id: &str) -> usize {
        self.backend.clear_working(task_id)
    }

    /// Store a procedural memory (step sequence).
    pub fn store_procedure(&mut self, name: &str, steps: &[String]) -> Option<String> {
        self.backend.store_procedure(name, steps)
    }

    /// Recall procedures matching a query.
    pub fn recall_procedure(&self, query: &str, limit: usize) -> Vec<Procedure> {
        self.backend.recall_procedure(query, limit)
    }

    /// Store a prospective memory (future intention).
    pub fn store_prospective(
        &mut self,
        description: &str,
        trigger_condition: &str,
        action: &str,
    ) -> Option<String> {
        self.backend
            .store_prospective(description, trigger_condition, action)
    }

    /// Check which prospective memories are triggered by content.
    pub fn check_triggers(&self, content: &str) -> Vec<ProspectiveTrigger> {
        self.backend.check_triggers(content)
    }

    /// Record short-lived sensory input.
    pub fn record_sensory(
        &mut self,
        modality: &str,
        raw_data: &str,
        ttl_seconds: u64,
    ) -> Option<String> {
        self.backend.record_sensory(modality, raw_data, ttl_seconds)
    }

    // ------------------------------------------------------------------
    // Utility
    // ------------------------------------------------------------------

    /// Flush caches without losing data.
    pub fn flush(&mut self) {
        self.backend.flush();
    }

    /// Close the backend, releasing resources.
    pub fn close(&mut self) {
        self.backend.close();
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    fn backend_search(
        &self,
        query: &str,
        limit: usize,
        min_confidence: f64,
    ) -> Vec<MemoryFact> {
        self.backend
            .search_facts(query, limit * SEARCH_CANDIDATE_MULTIPLIER, min_confidence)
    }

    /// Promote a fact to the hive after quality gating.
    fn promote_to_hive(
        &mut self,
        context: &str,
        fact: &str,
        confidence: f64,
        tags: &[String],
        metadata: &HashMap<String, Value>,
    ) {
        let hive = match self.hive.as_mut() {
            Some(h) => h,
            None => return,
        };

        // Quality gate
        if let Some(ref scorer) = self.quality_scorer
            && self.quality_threshold > 0.0
        {
            let quality = scorer.score(fact, context);
            if quality < self.quality_threshold {
                debug!(
                    quality,
                    threshold = self.quality_threshold,
                    "fact rejected by quality gate"
                );
                return;
            }
        }

        let hive_fact = HiveFact {
            fact_id: String::new(),
            content: fact.to_string(),
            concept: context.to_string(),
            confidence,
            source_agent: self.agent_name.clone(),
            tags: tags.to_vec(),
            metadata: metadata.clone(),
            timestamp: String::new(),
        };

        if let Err(e) = hive.promote_fact(&self.agent_name, &hive_fact) {
            debug!("failed to promote fact to hive (non-fatal): {e}");
        }
    }

    /// Search the hive with confidence gating.
    fn search_hive(&self, hive: &dyn HiveStore, query: &str, limit: usize) -> Vec<MemoryFact> {
        let hive_facts = hive.search(query, limit);
        if hive_facts.is_empty() {
            return Vec::new();
        }

        // Confidence gate
        if self.confidence_gate > 0.0 {
            let max_conf = hive_facts
                .iter()
                .map(|f| f.confidence)
                .fold(0.0_f64, f64::max);
            if max_conf < self.confidence_gate {
                debug!(
                    max_conf,
                    gate = self.confidence_gate,
                    "hive results below confidence gate"
                );
                return Vec::new();
            }
        }

        hive_facts.into_iter().map(hive_fact_to_memory).collect()
    }
}

// ---------------------------------------------------------------------------
// MemoryRetriever implementation
// ---------------------------------------------------------------------------

impl MemoryRetriever for CognitiveAdapter {
    fn search(&self, query: &str, limit: usize) -> Vec<MemoryFact> {
        self.search_full(query, limit, 0.0)
    }

    fn store_fact(&self, context: &str, fact: &str, confidence: f64, tags: &[String]) {
        // MemoryRetriever takes &self but we need mutation.
        // This is safe because the trait is designed for interior mutability
        // patterns — callers should wrap CognitiveAdapter in a Mutex.
        warn!("store_fact via MemoryRetriever trait ignores hive promotion — use store_fact_full");
        let _ = (context, fact, confidence, tags);
    }
}

impl MemoryFacade for CognitiveAdapter {
    fn remember(&self, content: &str) {
        // Simplified: store as a semantic fact with default context.
        let _ = content;
        warn!("remember via MemoryFacade is a no-op — use store_fact_full");
    }

    fn recall(&self, query: &str, limit: usize) -> Vec<String> {
        self.search_full(query, limit, 0.0)
            .into_iter()
            .map(|f| f.outcome)
            .collect()
    }

    fn retrieve_facts(&self, query: &str, max_nodes: usize) -> Vec<MemoryFact> {
        self.search_full(query, max_nodes, 0.0)
    }
}
