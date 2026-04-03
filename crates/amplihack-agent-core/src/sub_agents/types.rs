//! Shared types for the sub-agents module.
//!
//! Contains enums, structs, traits, constants, and the `rerank_facts_by_query`
//! utility used across all sub-agent components.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agentic_loop::MemoryFact;
use crate::error::AgentError;

// ---------------------------------------------------------------------------
// Retrieval constants (ported from Python retrieval_constants.py)
// ---------------------------------------------------------------------------

pub const AGGREGATION_FACTS_LIMIT: usize = 10;
pub const CONCEPT_DISPLAY_LIMIT: usize = 10;
pub const DEFAULT_TEMPORAL_INDEX: i64 = 999_999;
pub const ENTITY_DISPLAY_LIMIT: usize = 20;
pub const MAX_RETRIEVAL_LIMIT: usize = 500;
pub const MEMORY_AGENT_SMALL_KB_THRESHOLD: usize = 50;
pub const SEARCH_CANDIDATE_MULTIPLIER: usize = 3;
pub const SIMPLE_RETRIEVE_DEFAULT_LIMIT: usize = 30;
pub const TWO_PHASE_BROAD_CAP: usize = 200;

// ---------------------------------------------------------------------------
// SpecialistType
// ---------------------------------------------------------------------------

/// Types of specialist sub-agents that can be spawned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecialistType {
    Retrieval,
    Analysis,
    Synthesis,
    CodeGeneration,
    Research,
}

impl std::fmt::Display for SpecialistType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retrieval => write!(f, "retrieval"),
            Self::Analysis => write!(f, "analysis"),
            Self::Synthesis => write!(f, "synthesis"),
            Self::CodeGeneration => write!(f, "code_generation"),
            Self::Research => write!(f, "research"),
        }
    }
}

impl std::str::FromStr for SpecialistType {
    type Err = AgentError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "retrieval" => Ok(Self::Retrieval),
            "analysis" => Ok(Self::Analysis),
            "synthesis" => Ok(Self::Synthesis),
            "code_generation" => Ok(Self::CodeGeneration),
            "research" => Ok(Self::Research),
            other => Err(AgentError::ConfigError(format!(
                "unknown specialist type: {other}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// SpawnedAgent
// ---------------------------------------------------------------------------

/// Lifecycle status of a spawned sub-agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpawnedAgentStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Failed,
}

/// A spawned sub-agent and its lifecycle state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnedAgent {
    pub name: String,
    pub specialist_type: SpecialistType,
    pub task: String,
    pub parent_memory_path: String,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub status: SpawnedAgentStatus,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub elapsed_seconds: f64,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// TaskRoute
// ---------------------------------------------------------------------------

/// Describes how to route a task to sub-agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRoute {
    pub retrieval_strategy: String,
    pub needs_reasoning: bool,
    pub needs_teaching: bool,
    pub reasoning_type: String,
    pub parallel_retrieval: bool,
}

impl Default for TaskRoute {
    fn default() -> Self {
        Self {
            retrieval_strategy: "auto".to_string(),
            needs_reasoning: false,
            needs_teaching: false,
            reasoning_type: String::new(),
            parallel_retrieval: false,
        }
    }
}

// ---------------------------------------------------------------------------
// RetrievalStrategy
// ---------------------------------------------------------------------------

/// Retrieval strategy selection for different question types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalStrategy {
    EntityCentric,
    Temporal,
    Aggregation,
    FullText,
    SimpleAll,
    TwoPhase,
}

impl std::fmt::Display for RetrievalStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EntityCentric => write!(f, "entity_centric"),
            Self::Temporal => write!(f, "temporal"),
            Self::Aggregation => write!(f, "aggregation"),
            Self::FullText => write!(f, "full_text"),
            Self::SimpleAll => write!(f, "simple_all"),
            Self::TwoPhase => write!(f, "two_phase"),
        }
    }
}

// ---------------------------------------------------------------------------
// AggregationResult
// ---------------------------------------------------------------------------

/// Result of an aggregation query against the memory backend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregationResult {
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub count: Option<usize>,
    #[serde(default)]
    pub item_counts: HashMap<String, usize>,
}

// ---------------------------------------------------------------------------
// SubAgentMemory trait
// ---------------------------------------------------------------------------

/// Trait abstracting memory access for sub-agents.
///
/// Required methods cover keyword search and fact storage. Optional methods
/// (with default empty implementations) support aggregation and entity-based
/// retrieval for backends that provide those capabilities.
pub trait SubAgentMemory: Send + Sync {
    /// Keyword search returning up to `limit` results.
    fn search(&self, query: &str, limit: usize) -> Vec<MemoryFact>;

    /// Return all stored facts up to `limit`.
    fn get_all_facts(&self, limit: usize) -> Vec<MemoryFact>;

    /// Store a fact in memory.
    fn store_fact(&self, context: &str, fact: &str, confidence: f64, tags: &[String]);

    /// Execute an aggregation query. Returns empty result by default.
    fn execute_aggregation(&self, _op: &str, _entity_filter: &str) -> AggregationResult {
        AggregationResult::default()
    }

    /// Retrieve facts by entity name. Returns empty by default.
    fn retrieve_by_entity(&self, _entity: &str, _limit: usize) -> Vec<MemoryFact> {
        Vec::new()
    }

    /// Whether this backend supports aggregation queries.
    fn has_aggregation_support(&self) -> bool {
        false
    }

    /// Whether this backend supports entity-based retrieval.
    fn has_entity_support(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// rerank_facts_by_query
// ---------------------------------------------------------------------------

/// Rerank facts by token overlap with the query, returning up to `top_k`.
///
/// Scores each fact by the fraction of query tokens found in the fact's
/// `context` + `outcome` text (weight 0.7), boosted by the fact's
/// `confidence` field (weight 0.3).
pub fn rerank_facts_by_query(
    facts: &[MemoryFact],
    query: &str,
    top_k: Option<usize>,
) -> Vec<MemoryFact> {
    if facts.is_empty() || query.is_empty() {
        return facts.to_vec();
    }

    let query_lower = query.to_lowercase();
    let query_tokens: Vec<&str> = query_lower.split_whitespace().collect();
    if query_tokens.is_empty() {
        return facts.to_vec();
    }

    let mut scored: Vec<(f64, usize)> = facts
        .iter()
        .enumerate()
        .map(|(i, fact)| {
            let text = format!("{} {}", fact.context, fact.outcome).to_lowercase();
            let overlap = query_tokens.iter().filter(|qt| text.contains(*qt)).count();
            let token_score = overlap as f64 / query_tokens.len() as f64;
            let score = token_score * 0.7 + fact.confidence * 0.3;
            (score, i)
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let limit = top_k.unwrap_or(facts.len());
    scored
        .into_iter()
        .take(limit)
        .map(|(_, i)| facts[i].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fact(id: &str, context: &str, outcome: &str, confidence: f64) -> MemoryFact {
        MemoryFact {
            id: id.to_string(),
            context: context.to_string(),
            outcome: outcome.to_string(),
            confidence,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn specialist_type_display_and_parse() {
        for (s, t) in [
            ("retrieval", SpecialistType::Retrieval),
            ("analysis", SpecialistType::Analysis),
            ("synthesis", SpecialistType::Synthesis),
            ("code_generation", SpecialistType::CodeGeneration),
            ("research", SpecialistType::Research),
        ] {
            assert_eq!(t.to_string(), s);
            assert_eq!(s.parse::<SpecialistType>().unwrap(), t);
        }
        assert!("unknown".parse::<SpecialistType>().is_err());
    }

    #[test]
    fn specialist_type_serde() {
        let json = serde_json::to_string(&SpecialistType::CodeGeneration).unwrap();
        assert_eq!(json, r#""code_generation""#);
        let parsed: SpecialistType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SpecialistType::CodeGeneration);
    }

    #[test]
    fn spawned_agent_status_default() {
        assert_eq!(SpawnedAgentStatus::default(), SpawnedAgentStatus::Pending);
    }

    #[test]
    fn spawned_agent_serde_roundtrip() {
        let agent = SpawnedAgent {
            name: "test_sub_1".into(),
            specialist_type: SpecialistType::Retrieval,
            task: "Find facts".into(),
            parent_memory_path: "/data/mem".into(),
            result: Some("found 3 facts".into()),
            status: SpawnedAgentStatus::Completed,
            error: String::new(),
            elapsed_seconds: 1.5,
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&agent).unwrap();
        let parsed: SpawnedAgent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test_sub_1");
        assert_eq!(parsed.status, SpawnedAgentStatus::Completed);
        assert_eq!(parsed.result, Some("found 3 facts".into()));
    }

    #[test]
    fn task_route_default() {
        let route = TaskRoute::default();
        assert_eq!(route.retrieval_strategy, "auto");
        assert!(!route.needs_reasoning);
        assert!(!route.needs_teaching);
    }

    #[test]
    fn retrieval_strategy_display() {
        assert_eq!(RetrievalStrategy::TwoPhase.to_string(), "two_phase");
        assert_eq!(RetrievalStrategy::EntityCentric.to_string(), "entity_centric");
    }

    #[test]
    fn rerank_empty_input() {
        let result = rerank_facts_by_query(&[], "query", None);
        assert!(result.is_empty());
    }

    #[test]
    fn rerank_empty_query() {
        let facts = vec![make_fact("1", "ctx", "out", 0.5)];
        let result = rerank_facts_by_query(&facts, "", None);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn rerank_orders_by_relevance() {
        let facts = vec![
            make_fact("1", "unrelated", "nothing here", 0.9),
            make_fact("2", "Sarah Chen", "has a cat named Mochi", 0.8),
            make_fact("3", "cat breed", "tabby cat from shelter", 0.5),
        ];
        let result = rerank_facts_by_query(&facts, "What cat does Sarah have?", None);
        // Fact 2 mentions "cat" and "Sarah" — should rank higher
        assert_eq!(result[0].id, "2");
    }

    #[test]
    fn rerank_respects_top_k() {
        let facts = vec![
            make_fact("1", "a", "b", 0.5),
            make_fact("2", "c", "d", 0.5),
            make_fact("3", "e", "f", 0.5),
        ];
        let result = rerank_facts_by_query(&facts, "a", Some(2));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn aggregation_result_default() {
        let agg = AggregationResult::default();
        assert!(agg.items.is_empty());
        assert!(agg.count.is_none());
    }
}
