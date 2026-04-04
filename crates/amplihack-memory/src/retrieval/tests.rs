//! Tests for the retrieval subsystem.

use crate::retrieval::constants::*;
use crate::retrieval::router::*;
use crate::retrieval::scoring::*;
use crate::retrieval::strategies::*;
use crate::retrieval::types::*;
use std::collections::HashMap;

// ── Mock memory backend ─────────────────────────────────────────────

struct MockMemory {
    facts: Vec<Fact>,
    hierarchical: bool,
}

impl MockMemory {
    fn new(facts: Vec<Fact>) -> Self {
        Self {
            facts,
            hierarchical: false,
        }
    }

    fn hierarchical(facts: Vec<Fact>) -> Self {
        Self {
            facts,
            hierarchical: true,
        }
    }
}

impl MemorySearch for MockMemory {
    fn get_all_facts(&self, limit: usize, _query: &str) -> Vec<Fact> {
        self.facts.iter().take(limit).cloned().collect()
    }

    fn search(&self, query: &str, limit: usize) -> Vec<Fact> {
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

    fn retrieve_by_entity(&self, entity_name: &str, limit: usize) -> Vec<Fact> {
        let name = entity_name.to_lowercase();
        self.facts
            .iter()
            .filter(|f| {
                f.context.to_lowercase().contains(&name) || f.outcome.to_lowercase().contains(&name)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    fn search_by_concept(&self, keywords: &[String], limit: usize) -> Vec<Fact> {
        if let Some(kw) = keywords.first() {
            self.search(kw, limit)
        } else {
            Vec::new()
        }
    }

    fn get_statistics(&self) -> Option<MemoryStatistics> {
        Some(MemoryStatistics {
            total: Some(self.facts.len()),
            ..Default::default()
        })
    }

    fn supports_hierarchical(&self) -> bool {
        self.hierarchical
    }
}

fn make_facts(n: usize) -> Vec<Fact> {
    (0..n)
        .map(|i| {
            let mut f = Fact::new(format!("context-{i}"), format!("outcome-{i}"));
            f.experience_id = format!("exp-{i}");
            f.timestamp = format!("2024-01-{:02}", (i % 28) + 1);
            let mut meta = HashMap::new();
            meta.insert(
                "temporal_index".into(),
                serde_json::Value::Number(serde_json::Number::from(i as i64)),
            );
            f.metadata = meta;
            f
        })
        .collect()
}

// ── simple_retrieval ────────────────────────────────────────────────

#[test]
fn simple_retrieval_small_kb_returns_all() {
    let facts = make_facts(50);
    let mem = MockMemory::new(facts.clone());
    let (result, exhaustive) = simple_retrieval(&mem, "anything", false, None);
    assert!(exhaustive);
    assert_eq!(result.len(), 50);
}

#[test]
fn simple_retrieval_force_verbatim() {
    let facts = make_facts(2000);
    let mem = MockMemory::new(facts);
    let (result, exhaustive) = simple_retrieval(&mem, "anything", true, None);
    assert!(exhaustive);
    assert_eq!(result.len(), 2000);
}

#[test]
fn simple_retrieval_large_kb_uses_tiers() {
    // Use repeated contexts so summarisation actually groups & reduces.
    let facts: Vec<Fact> = (0..1500)
        .map(|i| {
            let group = i % 50; // 50 groups of 30 facts each
            let mut f = Fact::new(format!("group-{group}"), format!("outcome-{i}"));
            f.experience_id = format!("exp-{i}");
            let mut meta = HashMap::new();
            meta.insert(
                "temporal_index".into(),
                serde_json::Value::Number(serde_json::Number::from(i as i64)),
            );
            f.metadata = meta;
            f
        })
        .collect();
    let mem = MockMemory::new(facts);
    let (result, exhaustive) = simple_retrieval(&mem, "anything", false, None);
    assert!(!exhaustive);
    // Tiered + summarisation should reduce below 1500
    assert!(result.len() < 1500);
}

#[test]
fn simple_retrieval_pre_snapshot() {
    let snap = make_facts(100);
    let mem = MockMemory::new(vec![]);
    let (result, exhaustive) = simple_retrieval(&mem, "test", false, Some(&snap));
    assert!(exhaustive);
    assert_eq!(result.len(), 100);
}

// ── tiered_retrieval ────────────────────────────────────────────────

#[test]
fn tiered_retrieval_preserves_tier1() {
    let facts = make_facts(1500);
    let result = tiered_retrieval("test query", &facts);
    // Tier 1 is the last TIER1_VERBATIM_SIZE facts (verbatim)
    assert!(result.len() >= TIER1_VERBATIM_SIZE);
}

#[test]
fn tiered_retrieval_small_input() {
    let facts = make_facts(5);
    let result = tiered_retrieval("test", &facts);
    // Small input: all facts stay in tier 1
    assert_eq!(result.len(), 5);
}

// ── summarize_old_facts ─────────────────────────────────────────────

#[test]
fn summarize_groups_by_context() {
    let mut facts = Vec::new();
    for i in 0..10 {
        facts.push(Fact::new("Project Alpha", format!("detail {i}")));
    }
    for i in 0..3 {
        facts.push(Fact::new("Project Beta", format!("beta detail {i}")));
    }
    let summaries = summarize_old_facts(&facts, "entity");
    // Alpha has >2 facts → summary; Beta has 3 → summary
    let summary_count = summaries
        .iter()
        .filter(|f| f.context.starts_with("SUMMARY"))
        .count();
    assert!(summary_count >= 1);
}

#[test]
fn summarize_preserves_small_groups() {
    let facts = vec![
        Fact::new("only-one", "single fact"),
        Fact::new("pair", "fact A"),
        Fact::new("pair", "fact B"),
    ];
    let summaries = summarize_old_facts(&facts, "entity");
    // "only-one" group has 1 fact, "pair" has 2 → both kept verbatim
    assert_eq!(summaries.len(), 3);
}

// ── entity_retrieval ────────────────────────────────────────────────

#[test]
fn entity_retrieval_finds_names() {
    let facts = vec![
        Fact::new("Sarah Chen", "Works at lab"),
        Fact::new("General info", "Unrelated fact"),
    ];
    let mem = MockMemory::hierarchical(facts);
    let result = entity_retrieval(&mem, "What does Sarah Chen do?", false);
    assert!(!result.is_empty());
    assert!(result[0].context.contains("Sarah Chen"));
}

#[test]
fn entity_retrieval_non_hierarchical_empty() {
    let mem = MockMemory::new(vec![Fact::new("Sarah Chen", "x")]);
    let result = entity_retrieval(&mem, "Sarah Chen?", false);
    assert!(result.is_empty());
}

// ── concept_retrieval ───────────────────────────────────────────────

#[test]
fn concept_retrieval_filters_stop_words() {
    let facts = vec![
        Fact::new("programming", "Rust is fast"),
        Fact::new("general", "the cat sat"),
    ];
    let mem = MockMemory::hierarchical(facts);
    let result = concept_retrieval(&mem, "what is programming?", false);
    assert!(!result.is_empty());
}

// ── entity_linked_retrieval ─────────────────────────────────────────

#[test]
fn entity_linked_adds_id_facts() {
    let existing = vec![Fact::new("ctx", "existing fact")];
    let mut linked_fact = Fact::new("incident", "INC-2024-001 was critical");
    linked_fact.experience_id = "linked-1".into();
    let mem = MockMemory::new(vec![linked_fact]);
    let result = entity_linked_retrieval(&mem, "Tell me about INC-2024-001", &existing, false);
    assert!(result.len() > existing.len());
}

#[test]
fn entity_linked_no_ids_passthrough() {
    let existing = make_facts(3);
    let mem = MockMemory::new(vec![]);
    let result = entity_linked_retrieval(&mem, "No IDs here", &existing, false);
    assert_eq!(result.len(), existing.len());
}

// ── multi_entity_retrieval ──────────────────────────────────────────

#[test]
fn multi_entity_needs_two_entities() {
    let existing = make_facts(2);
    let mem = MockMemory::new(vec![]);
    // Only 1 entity → passthrough
    let result = multi_entity_retrieval(&mem, "Tell me about Alice", &existing, false);
    assert_eq!(result.len(), existing.len());
}

// ── infrastructure_relation_retrieval ────────────────────────────────

#[test]
fn infra_retrieval_needs_subnet() {
    let mem = MockMemory::new(vec![]);
    let result = infrastructure_relation_retrieval(&mem, "tell me about the project", &[], false);
    assert!(result.is_empty());
}

#[test]
fn infra_retrieval_extracts_subnet_names() {
    let existing = vec![Fact::new(
        "network",
        "The subnet named prod-web is in 10.0.0.0/24",
    )];
    let mut related = Fact::new("prod-web", "prod-web has 256 addresses");
    related.experience_id = "net-1".into();
    let mem = MockMemory::hierarchical(vec![related]);
    let result = infrastructure_relation_retrieval(&mem, "what subnet is used?", &existing, false);
    // Should find the prod-web related fact
    assert!(!result.is_empty() || existing.is_empty());
}

// ── aggregation_retrieval ───────────────────────────────────────────

#[test]
fn aggregation_non_hierarchical_falls_back() {
    let facts = make_facts(5);
    let mem = MockMemory::new(facts);
    let result = aggregation_retrieval(&mem, "how many projects?");
    // Falls back to simple retrieval
    assert!(!result.is_empty());
}

// ── filter_facts_by_source_reference ────────────────────────────────

#[test]
fn source_filter_finds_matching() {
    let mut f = Fact::new("ctx", "content");
    f.metadata.insert(
        "source_label".into(),
        serde_json::Value::String("athlete achievements".into()),
    );
    let facts = vec![f, Fact::new("other", "no source")];
    let result =
        filter_facts_by_source_reference("mentioned in the athlete achievements article?", &facts);
    assert_eq!(result.len(), 1);
}

#[test]
fn source_filter_no_pattern_empty() {
    let facts = make_facts(3);
    let result = filter_facts_by_source_reference("just a question", &facts);
    assert!(result.is_empty());
}

// ── extract_entity_ids ──────────────────────────────────────────────

#[test]
fn extract_entity_ids_finds_patterns() {
    let ids = extract_entity_ids("Check INC-2024-001 and CVE-2024-3094 please");
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"INC-2024-001".to_string()));
    assert!(ids.contains(&"CVE-2024-3094".to_string()));
}

#[test]
fn extract_entity_ids_none() {
    let ids = extract_entity_ids("no structured IDs here");
    assert!(ids.is_empty());
}

// ── IntentKind ──────────────────────────────────────────────────────

#[test]
fn intent_simple_classification() {
    assert!(IntentKind::SimpleRecall.is_simple());
    assert!(IntentKind::MultiSourceSynthesis.is_simple());
    assert!(!IntentKind::MetaMemory.is_simple());
}

#[test]
fn intent_aggregation_classification() {
    assert!(IntentKind::MetaMemory.is_aggregation());
    assert!(!IntentKind::SimpleRecall.is_aggregation());
}

// ── Fact helpers ────────────────────────────────────────────────────

#[test]
fn fact_dedup_key_uses_experience_id() {
    let mut f = Fact::new("ctx", "out");
    f.experience_id = "exp-123".into();
    assert_eq!(f.dedup_key(), "exp-123");
}

#[test]
fn fact_dedup_key_fallback() {
    let f = Fact::new("ctx", "out");
    assert_eq!(f.dedup_key(), "ctx::out");
}

#[test]
fn fact_temporal_sort_key() {
    let mut f = Fact::new("ctx", "out");
    f.metadata.insert(
        "temporal_index".into(),
        serde_json::Value::Number(serde_json::Number::from(42)),
    );
    f.timestamp = "2024-01-15".into();
    assert_eq!(f.temporal_sort_key(), (42, "2024-01-15".to_string()));
}

#[test]
fn fact_summary_builder() {
    let f = Fact::summary("MyGroup", 5, "combined text", "entity");
    assert_eq!(f.context, "SUMMARY (MyGroup)");
    assert!(f.outcome.contains("5 facts"));
    assert_eq!(f.confidence, 0.7);
    assert!(f.tags.contains(&"summary".to_string()));
}

// ── MemoryStatistics ────────────────────────────────────────────────

#[test]
fn memory_stats_estimated_total() {
    let s = MemoryStatistics {
        total_experiences: Some(100),
        ..Default::default()
    };
    assert_eq!(s.estimated_total(), Some(100));

    let s2 = MemoryStatistics {
        semantic_nodes: Some(50),
        episodic_nodes: Some(30),
        ..Default::default()
    };
    assert_eq!(s2.estimated_total(), Some(80));
}

// ── supplement_simple_retrieval ──────────────────────────────────────

#[test]
fn supplement_adds_entity_facts() {
    let existing = vec![Fact::new("general", "some context")];
    let mut entity_fact = Fact::new("Sarah Chen", "Works at lab");
    entity_fact.experience_id = "ent-1".into();
    let mem = MockMemory::hierarchical(vec![entity_fact]);
    let result = supplement_simple_retrieval(&mem, "Tell me about Sarah Chen", &existing, false);
    assert!(result.len() > existing.len());
}

// ── Scoring edge cases ──────────────────────────────────────────────

#[test]
fn ngram_case_insensitive() {
    let score = ngram_overlap_score("Hello World", "hello world");
    assert!((score - 1.0).abs() < f64::EPSILON);
}

#[test]
fn merge_preserves_order() {
    let a = vec![Fact::new("first", "a")];
    let b = vec![Fact::new("second", "b")];
    let merged = merge_facts(a, b);
    assert_eq!(merged[0].context, "first");
    assert_eq!(merged[1].context, "second");
}
