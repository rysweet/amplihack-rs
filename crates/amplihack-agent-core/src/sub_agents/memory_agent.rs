//! Memory Agent: Specialized retrieval strategy selection.
//!
//! Decides **how** to retrieve facts based on question characteristics,
//! then delegates to the appropriate memory backend method.

use std::cmp::min;
use std::collections::HashMap;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use tracing::debug;

use crate::agentic_loop::MemoryFact;

use super::types::{
    AGGREGATION_FACTS_LIMIT, CONCEPT_DISPLAY_LIMIT, DEFAULT_TEMPORAL_INDEX, ENTITY_DISPLAY_LIMIT,
    MAX_RETRIEVAL_LIMIT, MEMORY_AGENT_SMALL_KB_THRESHOLD, RetrievalStrategy,
    SEARCH_CANDIDATE_MULTIPLIER, SIMPLE_RETRIEVE_DEFAULT_LIMIT, SubAgentMemory,
    TWO_PHASE_BROAD_CAP, rerank_facts_by_query,
};

static MULTI_WORD_PROPER_NOUN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b([A-Z][a-z]+(?:\s+[A-Z][a-z]+)+)\b").expect("valid regex"));

static POSSESSIVE_PROPER_NOUN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b([A-Z][a-z]+)'s\b").expect("valid regex"));

// ---------------------------------------------------------------------------
// MemoryAgent
// ---------------------------------------------------------------------------

/// Specialized memory retrieval agent.
///
/// Selects the optimal retrieval strategy based on question characteristics
/// and delegates to the appropriate [`SubAgentMemory`] method.
pub struct MemoryAgent<M> {
    memory: M,
    #[allow(dead_code)]
    agent_name: String,
}

impl<M: SubAgentMemory> MemoryAgent<M> {
    pub fn new(memory: M, agent_name: impl Into<String>) -> Self {
        Self {
            memory,
            agent_name: agent_name.into(),
        }
    }

    /// Borrow the underlying memory backend.
    pub fn memory(&self) -> &M {
        &self.memory
    }

    /// Select the best retrieval strategy for a question.
    pub fn select_strategy(
        &self,
        question: &str,
        intent: &HashMap<String, Value>,
    ) -> RetrievalStrategy {
        let intent_type = intent
            .get("intent")
            .and_then(|v| v.as_str())
            .unwrap_or("simple_recall");

        if intent_type == "meta_memory" {
            return RetrievalStrategy::Aggregation;
        }

        if intent
            .get("needs_temporal")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return RetrievalStrategy::Temporal;
        }

        let kb_size = self.get_kb_size();

        if kb_size <= MEMORY_AGENT_SMALL_KB_THRESHOLD {
            return RetrievalStrategy::SimpleAll;
        }

        if has_entity_reference(question) {
            return RetrievalStrategy::EntityCentric;
        }

        if kb_size > MEMORY_AGENT_SMALL_KB_THRESHOLD {
            return RetrievalStrategy::TwoPhase;
        }

        RetrievalStrategy::FullText
    }

    /// Retrieve facts using the optimal strategy for the question.
    pub fn retrieve(
        &self,
        question: &str,
        intent: &HashMap<String, Value>,
        max_facts: usize,
    ) -> Vec<MemoryFact> {
        let strategy = self.select_strategy(question, intent);
        debug!(strategy = %strategy, question = &question[..question.len().min(60)], "MemoryAgent");

        match strategy {
            RetrievalStrategy::Aggregation => self.aggregation_retrieve(question, intent),
            RetrievalStrategy::SimpleAll => self.simple_all_retrieve(question, max_facts),
            RetrievalStrategy::EntityCentric => {
                let facts = self.entity_retrieve(question, max_facts);
                if facts.is_empty() {
                    // Fall through to two-phase if entity retrieval finds nothing
                    self.two_phase_retrieve(question, max_facts)
                } else {
                    facts
                }
            }
            RetrievalStrategy::Temporal => self.temporal_retrieve(question, max_facts),
            RetrievalStrategy::TwoPhase => self.two_phase_retrieve(question, max_facts),
            RetrievalStrategy::FullText => self.full_text_retrieve(question, max_facts),
        }
    }

    fn get_kb_size(&self) -> usize {
        self.memory.get_all_facts(MAX_RETRIEVAL_LIMIT).len()
    }

    fn aggregation_retrieve(
        &self,
        question: &str,
        _intent: &HashMap<String, Value>,
    ) -> Vec<MemoryFact> {
        if !self.memory.has_aggregation_support() {
            return self.simple_all_retrieve(question, SIMPLE_RETRIEVE_DEFAULT_LIMIT);
        }
        let q_lower = question.to_lowercase();
        let mut results: Vec<MemoryFact> = Vec::new();
        let entity_type = ["project", "people", "person", "team", "member"]
            .iter()
            .find(|kw| q_lower.contains(*kw))
            .copied()
            .unwrap_or("");

        if entity_type == "project" {
            let agg = self.memory.execute_aggregation("list_concepts", "project");
            if !agg.items.is_empty() {
                results.push(agg_fact(
                    "agg_project",
                    "Meta-memory: Project count",
                    &format!(
                        "There are {} distinct project-related concepts: {}",
                        agg.items.len(),
                        agg.items.join(", ")
                    ),
                ));
            }
        }
        if matches!(entity_type, "people" | "person" | "member" | "team") {
            let agg = self.memory.execute_aggregation("list_entities", "");
            if !agg.items.is_empty() {
                results.push(agg_fact(
                    "agg_entities",
                    "Meta-memory: Entity list",
                    &format!(
                        "There are {} distinct entities: {}",
                        agg.items.len(),
                        agg.items.join(", ")
                    ),
                ));
            }
        }
        if results.is_empty() {
            let entity_agg = self.memory.execute_aggregation("list_entities", "");
            let concept_agg = self.memory.execute_aggregation("count_by_concept", "");
            let total_agg = self.memory.execute_aggregation("count_total", "");
            let mut parts = Vec::new();
            if let Some(count) = total_agg.count {
                parts.push(format!("Total facts: {count}"));
            }
            if !entity_agg.items.is_empty() {
                let display: Vec<&str> = entity_agg
                    .items
                    .iter()
                    .take(ENTITY_DISPLAY_LIMIT)
                    .map(|s| s.as_str())
                    .collect();
                parts.push(format!(
                    "Entities ({}): {}",
                    entity_agg.items.len(),
                    display.join(", ")
                ));
            }
            if !concept_agg.item_counts.is_empty() {
                let mut top: Vec<(&str, &usize)> = concept_agg
                    .item_counts
                    .iter()
                    .map(|(k, v)| (k.as_str(), v))
                    .collect();
                top.sort_by(|a, b| b.1.cmp(a.1));
                let display: Vec<String> = top
                    .iter()
                    .take(CONCEPT_DISPLAY_LIMIT)
                    .map(|(c, n)| format!("{c} ({n})"))
                    .collect();
                parts.push(format!("Concepts: {}", display.join(", ")));
            }
            if !parts.is_empty() {
                results.push(agg_fact(
                    "agg_summary",
                    "Meta-memory summary",
                    &parts.join(". "),
                ));
            }
        }
        let regular = self.simple_all_retrieve(question, AGGREGATION_FACTS_LIMIT);
        results.extend(regular.into_iter().take(AGGREGATION_FACTS_LIMIT));
        results
    }

    fn simple_all_retrieve(&self, question: &str, max_facts: usize) -> Vec<MemoryFact> {
        let facts = self.memory.get_all_facts(max_facts);
        rerank_facts_by_query(&facts, question, None)
    }

    fn entity_retrieve(&self, question: &str, max_facts: usize) -> Vec<MemoryFact> {
        if !self.memory.has_entity_support() {
            return Vec::new();
        }
        let mut candidates: Vec<String> = MULTI_WORD_PROPER_NOUN
            .find_iter(question)
            .map(|m| m.as_str().to_string())
            .collect();
        candidates.extend(
            POSSESSIVE_PROPER_NOUN
                .captures_iter(question)
                .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
        );
        let mut all_facts: Vec<MemoryFact> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for candidate in &candidates {
            for fact in self.memory.retrieve_by_entity(candidate, max_facts) {
                if !fact.id.is_empty() && seen.insert(fact.id.clone()) {
                    all_facts.push(fact);
                }
            }
        }
        if all_facts.is_empty() {
            Vec::new()
        } else {
            rerank_facts_by_query(&all_facts, question, None)
        }
    }

    fn temporal_retrieve(&self, question: &str, max_facts: usize) -> Vec<MemoryFact> {
        let mut facts = self.simple_all_retrieve(question, max_facts);
        facts.sort_by(|a, b| {
            let t_a = temporal_index(a);
            let t_b = temporal_index(b);
            t_a.cmp(&t_b)
        });
        facts
    }

    fn two_phase_retrieve(&self, question: &str, max_facts: usize) -> Vec<MemoryFact> {
        let broad_limit = min(max_facts * SEARCH_CANDIDATE_MULTIPLIER, TWO_PHASE_BROAD_CAP);
        let mut candidates = self.memory.search(question, broad_limit);

        if candidates.is_empty() {
            candidates = self.memory.get_all_facts(broad_limit);
        }

        if candidates.is_empty() {
            return Vec::new();
        }

        rerank_facts_by_query(&candidates, question, Some(max_facts))
    }

    fn full_text_retrieve(&self, question: &str, max_facts: usize) -> Vec<MemoryFact> {
        let results = self.memory.search(question, max_facts);
        if results.is_empty() {
            Vec::new()
        } else {
            rerank_facts_by_query(&results, question, None)
        }
    }
}

/// Check if a question references a specific entity (proper noun).
fn has_entity_reference(question: &str) -> bool {
    MULTI_WORD_PROPER_NOUN.is_match(question) || POSSESSIVE_PROPER_NOUN.is_match(question)
}

/// Extract temporal index from a fact's metadata.
fn temporal_index(fact: &MemoryFact) -> i64 {
    fact.metadata
        .get("temporal_index")
        .and_then(|v| v.as_i64())
        .unwrap_or(DEFAULT_TEMPORAL_INDEX)
}

fn agg_fact(id: &str, context: &str, outcome: &str) -> MemoryFact {
    MemoryFact {
        id: id.into(),
        context: context.into(),
        outcome: outcome.into(),
        confidence: 1.0,
        metadata: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct MockMemory {
        facts: Vec<MemoryFact>,
    }

    impl MockMemory {
        fn empty() -> Self {
            Self { facts: Vec::new() }
        }
        fn with_facts(facts: Vec<MemoryFact>) -> Self {
            Self { facts }
        }
    }

    impl SubAgentMemory for MockMemory {
        fn search(&self, query: &str, limit: usize) -> Vec<MemoryFact> {
            let q = query.to_lowercase();
            self.facts
                .iter()
                .filter(|f| {
                    f.context.to_lowercase().contains(&q) || f.outcome.to_lowercase().contains(&q)
                })
                .take(limit)
                .cloned()
                .collect()
        }
        fn get_all_facts(&self, limit: usize) -> Vec<MemoryFact> {
            self.facts.iter().take(limit).cloned().collect()
        }
        fn store_fact(&self, _: &str, _: &str, _: f64, _: &[String]) {}
    }

    fn fact(id: &str, ctx: &str, outcome: &str) -> MemoryFact {
        MemoryFact {
            id: id.into(),
            context: ctx.into(),
            outcome: outcome.into(),
            confidence: 0.8,
            metadata: HashMap::new(),
        }
    }

    fn intent(s: &str) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert("intent".into(), Value::String(s.into()));
        m
    }

    #[test]
    fn strategy_meta_memory() {
        let agent = MemoryAgent::new(MockMemory::empty(), "test");
        let strategy = agent.select_strategy("How many?", &intent("meta_memory"));
        assert_eq!(strategy, RetrievalStrategy::Aggregation);
    }

    #[test]
    fn strategy_temporal() {
        let agent = MemoryAgent::new(MockMemory::empty(), "test");
        let mut i = intent("simple_recall");
        i.insert("needs_temporal".into(), Value::Bool(true));
        assert_eq!(
            agent.select_strategy("When?", &i),
            RetrievalStrategy::Temporal
        );
    }

    #[test]
    fn strategy_small_kb() {
        let facts: Vec<MemoryFact> = (0..10).map(|i| fact(&i.to_string(), "c", "o")).collect();
        let agent = MemoryAgent::new(MockMemory::with_facts(facts), "test");
        assert_eq!(
            agent.select_strategy("question", &intent("simple_recall")),
            RetrievalStrategy::SimpleAll
        );
    }

    #[test]
    fn strategy_entity_centric() {
        let facts: Vec<MemoryFact> = (0..100).map(|i| fact(&i.to_string(), "c", "o")).collect();
        let agent = MemoryAgent::new(MockMemory::with_facts(facts), "test");
        assert_eq!(
            agent.select_strategy("What is Sarah Chen's hobby?", &intent("simple_recall")),
            RetrievalStrategy::EntityCentric
        );
    }

    #[test]
    fn strategy_two_phase_large_kb_no_entity() {
        let facts: Vec<MemoryFact> = (0..100).map(|i| fact(&i.to_string(), "c", "o")).collect();
        let agent = MemoryAgent::new(MockMemory::with_facts(facts), "test");
        assert_eq!(
            agent.select_strategy("what happened?", &intent("simple_recall")),
            RetrievalStrategy::TwoPhase
        );
    }

    #[test]
    fn retrieve_simple_all() {
        let facts = vec![
            fact("1", "Sarah", "has a cat"),
            fact("2", "Bob", "likes dogs"),
        ];
        let agent = MemoryAgent::new(MockMemory::with_facts(facts), "test");
        let result = agent.retrieve("pets?", &intent("simple_recall"), 10);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn has_entity_reference_proper_nouns() {
        assert!(has_entity_reference("What is Sarah Chen doing?"));
        assert!(has_entity_reference("Tell me about Fatima's project"));
        assert!(!has_entity_reference("what happened yesterday?"));
    }

    #[test]
    fn temporal_index_extraction() {
        let mut f = fact("1", "c", "o");
        f.metadata
            .insert("temporal_index".into(), serde_json::json!(42));
        assert_eq!(temporal_index(&f), 42);

        let f2 = fact("2", "c", "o");
        assert_eq!(temporal_index(&f2), DEFAULT_TEMPORAL_INDEX);
    }
}
