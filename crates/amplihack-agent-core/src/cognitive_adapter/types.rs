//! Traits and types for cognitive adapter backends and hive stores.
//!
//! Defines the seams that `CognitiveAdapter` uses to abstract over
//! CognitiveMemory vs HierarchicalMemory and optional hive stores.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agentic_loop::types::MemoryFact;

// ---------------------------------------------------------------------------
// BackendKind
// ---------------------------------------------------------------------------

/// Which memory backend flavour is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Full 6-type cognitive memory.
    Cognitive,
    /// Simplified hierarchical fallback.
    Hierarchical,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cognitive => "cognitive",
            Self::Hierarchical => "hierarchical",
        }
    }
}

// ---------------------------------------------------------------------------
// CognitiveBackend — primary memory backend trait
// ---------------------------------------------------------------------------

/// Trait abstracting over CognitiveMemory and HierarchicalMemory.
///
/// Concrete implementations live in the memory crate; tests use mocks.
pub trait CognitiveBackend: Send + Sync {
    /// Which kind of backend this is.
    fn kind(&self) -> BackendKind;

    // -- Semantic / fact storage --

    /// Store a fact, returning a node ID.
    fn store_fact(
        &mut self,
        concept: &str,
        content: &str,
        confidence: f64,
        source_id: &str,
        tags: &[String],
        metadata: &HashMap<String, Value>,
    ) -> String;

    /// Search facts by query, returning up to `limit` results above
    /// `min_confidence`.
    fn search_facts(
        &self,
        query: &str,
        limit: usize,
        min_confidence: f64,
    ) -> Vec<MemoryFact>;

    /// Return all stored facts (up to `limit`).
    fn get_all_facts(&self, limit: usize) -> Vec<MemoryFact>;

    // -- Working memory --

    /// Push a slot to working memory. Returns slot ID.
    fn push_working(
        &mut self,
        _slot_type: &str,
        _content: &str,
        _task_id: &str,
        _relevance: f64,
    ) -> Option<String> {
        None
    }

    /// Get working memory slots for a task.
    fn get_working(&self, _task_id: &str) -> Vec<WorkingSlot> {
        Vec::new()
    }

    /// Clear working memory for a task. Returns number cleared.
    fn clear_working(&mut self, _task_id: &str) -> usize {
        0
    }

    // -- Procedural memory --

    /// Store a named procedure (step sequence).
    fn store_procedure(
        &mut self,
        _name: &str,
        _steps: &[String],
    ) -> Option<String> {
        None
    }

    /// Recall procedures matching a query.
    fn recall_procedure(&self, _query: &str, _limit: usize) -> Vec<Procedure> {
        Vec::new()
    }

    // -- Prospective memory --

    /// Store a future intention with a trigger condition.
    fn store_prospective(
        &mut self,
        _description: &str,
        _trigger_condition: &str,
        _action: &str,
    ) -> Option<String> {
        None
    }

    /// Check which prospective memories are triggered by content.
    fn check_triggers(&self, _content: &str) -> Vec<ProspectiveTrigger> {
        Vec::new()
    }

    // -- Sensory memory --

    /// Record short-lived sensory input with a TTL.
    fn record_sensory(
        &mut self,
        _modality: &str,
        _raw_data: &str,
        _ttl_seconds: u64,
    ) -> Option<String> {
        None
    }

    // -- Episodic memory --

    /// Store an episode (raw source content).
    fn store_episode(&mut self, content: &str, source_label: &str) -> String;

    // -- Utility --

    /// Return statistics about the memory store.
    fn get_statistics(&self) -> HashMap<String, Value>;

    /// Flush caches without losing data.
    fn flush(&mut self) {}

    /// Close the backend, releasing resources.
    fn close(&mut self) {}
}

// ---------------------------------------------------------------------------
// Cognitive-specific types
// ---------------------------------------------------------------------------

/// A working-memory slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingSlot {
    pub id: String,
    pub slot_type: String,
    pub content: String,
    pub task_id: String,
    pub relevance: f64,
}

/// A stored procedure (step sequence).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Procedure {
    pub id: String,
    pub name: String,
    pub steps: Vec<String>,
}

/// A triggered prospective memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProspectiveTrigger {
    pub id: String,
    pub description: String,
    pub trigger_condition: String,
    pub action: String,
}

// ---------------------------------------------------------------------------
// HiveFact
// ---------------------------------------------------------------------------

/// A fact originating from the shared hive store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiveFact {
    pub fact_id: String,
    pub content: String,
    pub concept: String,
    pub confidence: f64,
    pub source_agent: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
    #[serde(default)]
    pub timestamp: String,
}

// ---------------------------------------------------------------------------
// HiveStore — shared memory trait
// ---------------------------------------------------------------------------

/// Trait for the shared hive store used in fact federation.
pub trait HiveStore: Send + Sync {
    /// Promote a local fact into the shared hive.
    fn promote_fact(&mut self, agent_name: &str, fact: &HiveFact) -> Result<(), String>;

    /// Search the hive for facts matching `query`.
    fn search(&self, query: &str, limit: usize) -> Vec<HiveFact>;

    /// Return all hive facts (up to `limit`).
    fn get_all_facts(&self, limit: usize) -> Vec<HiveFact>;

    /// Register an agent with the hive.
    fn register_agent(&mut self, _agent_name: &str) {}
}

// ---------------------------------------------------------------------------
// QualityScorer
// ---------------------------------------------------------------------------

/// Scores content quality for the promotion gate.
pub trait QualityScorer: Send + Sync {
    /// Score `content` in `context`. Returns a value in `[0.0, 1.0]`.
    fn score(&self, content: &str, context: &str) -> f64;
}

// ---------------------------------------------------------------------------
// CognitiveAdapterConfig
// ---------------------------------------------------------------------------

/// Configuration for constructing a `CognitiveAdapter`.
#[derive(Debug, Clone)]
pub struct CognitiveAdapterConfig {
    pub agent_name: String,
    pub quality_threshold: f64,
    pub confidence_gate: f64,
    pub enable_query_expansion: bool,
}

impl CognitiveAdapterConfig {
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
            quality_threshold: super::constants::DEFAULT_QUALITY_THRESHOLD,
            confidence_gate: super::constants::DEFAULT_CONFIDENCE_GATE,
            enable_query_expansion: false,
        }
    }

    pub fn with_quality_threshold(mut self, v: f64) -> Self {
        self.quality_threshold = v;
        self
    }

    pub fn with_confidence_gate(mut self, v: f64) -> Self {
        self.confidence_gate = v;
        self
    }

    pub fn with_query_expansion(mut self, v: bool) -> Self {
        self.enable_query_expansion = v;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kind_as_str() {
        assert_eq!(BackendKind::Cognitive.as_str(), "cognitive");
        assert_eq!(BackendKind::Hierarchical.as_str(), "hierarchical");
    }

    #[test]
    fn config_defaults() {
        let cfg = CognitiveAdapterConfig::new("agent1");
        assert_eq!(cfg.agent_name, "agent1");
        assert!((cfg.quality_threshold - 0.3).abs() < f64::EPSILON);
        assert!((cfg.confidence_gate - 0.3).abs() < f64::EPSILON);
        assert!(!cfg.enable_query_expansion);
    }

    #[test]
    fn config_builder() {
        let cfg = CognitiveAdapterConfig::new("a")
            .with_quality_threshold(0.5)
            .with_confidence_gate(0.7)
            .with_query_expansion(true);
        assert!((cfg.quality_threshold - 0.5).abs() < f64::EPSILON);
        assert!((cfg.confidence_gate - 0.7).abs() < f64::EPSILON);
        assert!(cfg.enable_query_expansion);
    }

    #[test]
    fn hive_fact_default_fields() {
        let f = HiveFact {
            fact_id: String::new(),
            content: "test".into(),
            concept: "ctx".into(),
            confidence: 0.9,
            source_agent: "a".into(),
            tags: vec![],
            metadata: HashMap::new(),
            timestamp: String::new(),
        };
        assert!(f.tags.is_empty());
        assert!(f.metadata.is_empty());
    }
}
