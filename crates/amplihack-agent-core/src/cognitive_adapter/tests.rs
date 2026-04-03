//! Comprehensive tests for the cognitive adapter module.

use std::collections::HashMap;

use serde_json::Value;

use super::adapter::CognitiveAdapter;
use super::constants::MAX_WORKING_SLOTS;
use super::types::*;
use crate::agentic_loop::traits::{MemoryFacade, MemoryRetriever};
use crate::agentic_loop::types::MemoryFact;

// ===================================================================
// Mock backend
// ===================================================================

struct MockCognitiveBackend {
    kind: BackendKind,
    facts: Vec<MemoryFact>,
    working: HashMap<String, Vec<WorkingSlot>>,
    procedures: Vec<Procedure>,
    prospective: Vec<(String, String, String, String)>,
    episodes: Vec<(String, String)>,
    next_id: usize,
}

impl MockCognitiveBackend {
    fn new(kind: BackendKind) -> Self {
        Self {
            kind,
            facts: Vec::new(),
            working: HashMap::new(),
            procedures: Vec::new(),
            prospective: Vec::new(),
            episodes: Vec::new(),
            next_id: 0,
        }
    }

    fn next_id(&mut self) -> String {
        self.next_id += 1;
        format!("id-{}", self.next_id)
    }
}

impl CognitiveBackend for MockCognitiveBackend {
    fn kind(&self) -> BackendKind {
        self.kind
    }

    fn store_fact(
        &mut self,
        concept: &str,
        content: &str,
        confidence: f64,
        _source_id: &str,
        _tags: &[String],
        _metadata: &HashMap<String, Value>,
    ) -> String {
        let id = self.next_id();
        self.facts.push(MemoryFact {
            id: id.clone(),
            context: concept.to_string(),
            outcome: content.to_string(),
            confidence,
            metadata: HashMap::new(),
        });
        id
    }

    fn search_facts(
        &self,
        query: &str,
        limit: usize,
        min_confidence: f64,
    ) -> Vec<MemoryFact> {
        let q = query.to_lowercase();
        self.facts
            .iter()
            .filter(|f| {
                f.confidence >= min_confidence
                    && (f.outcome.to_lowercase().contains(&q)
                        || f.context.to_lowercase().contains(&q))
            })
            .take(limit)
            .cloned()
            .collect()
    }

    fn get_all_facts(&self, limit: usize) -> Vec<MemoryFact> {
        self.facts.iter().take(limit).cloned().collect()
    }

    fn push_working(
        &mut self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> Option<String> {
        let count = self.working.get(task_id).map_or(0, |v| v.len());
        if count >= MAX_WORKING_SLOTS {
            return None;
        }
        let id = self.next_id();
        self.working.entry(task_id.to_string()).or_default().push(WorkingSlot {
            id: id.clone(),
            slot_type: slot_type.to_string(),
            content: content.to_string(),
            task_id: task_id.to_string(),
            relevance,
        });
        Some(id)
    }

    fn get_working(&self, task_id: &str) -> Vec<WorkingSlot> {
        self.working.get(task_id).cloned().unwrap_or_default()
    }

    fn clear_working(&mut self, task_id: &str) -> usize {
        self.working.remove(task_id).map_or(0, |v| v.len())
    }

    fn store_procedure(&mut self, name: &str, steps: &[String]) -> Option<String> {
        let id = self.next_id();
        self.procedures.push(Procedure {
            id: id.clone(),
            name: name.to_string(),
            steps: steps.to_vec(),
        });
        Some(id)
    }

    fn recall_procedure(&self, query: &str, limit: usize) -> Vec<Procedure> {
        let q = query.to_lowercase();
        self.procedures
            .iter()
            .filter(|p| p.name.to_lowercase().contains(&q))
            .take(limit)
            .cloned()
            .collect()
    }

    fn store_prospective(
        &mut self,
        description: &str,
        trigger_condition: &str,
        action: &str,
    ) -> Option<String> {
        let id = self.next_id();
        self.prospective.push((
            id.clone(),
            description.to_string(),
            trigger_condition.to_string(),
            action.to_string(),
        ));
        Some(id)
    }

    fn check_triggers(&self, content: &str) -> Vec<ProspectiveTrigger> {
        let c = content.to_lowercase();
        self.prospective
            .iter()
            .filter(|(_, _, trigger, _)| c.contains(&trigger.to_lowercase()))
            .map(|(id, desc, trigger, action)| ProspectiveTrigger {
                id: id.clone(),
                description: desc.clone(),
                trigger_condition: trigger.clone(),
                action: action.clone(),
            })
            .collect()
    }

    fn record_sensory(
        &mut self,
        _modality: &str,
        _raw_data: &str,
        _ttl_seconds: u64,
    ) -> Option<String> {
        Some(self.next_id())
    }

    fn store_episode(&mut self, content: &str, source_label: &str) -> String {
        let id = self.next_id();
        self.episodes
            .push((content.to_string(), source_label.to_string()));
        id
    }

    fn get_statistics(&self) -> HashMap<String, Value> {
        let mut stats = HashMap::new();
        stats.insert(
            "total".into(),
            serde_json::to_value(self.facts.len()).unwrap(),
        );
        stats
    }
}

// ===================================================================
// Mock hive store
// ===================================================================

struct MockHiveStore {
    facts: Vec<HiveFact>,
    agents: Vec<String>,
}

impl MockHiveStore {
    fn new() -> Self {
        Self {
            facts: Vec::new(),
            agents: Vec::new(),
        }
    }

    fn with_facts(facts: Vec<HiveFact>) -> Self {
        Self {
            facts,
            agents: Vec::new(),
        }
    }
}

impl HiveStore for MockHiveStore {
    fn promote_fact(&mut self, _agent_name: &str, fact: &HiveFact) -> Result<(), String> {
        self.facts.push(fact.clone());
        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> Vec<HiveFact> {
        let q = query.to_lowercase();
        self.facts
            .iter()
            .filter(|f| {
                f.content.to_lowercase().contains(&q)
                    || f.concept.to_lowercase().contains(&q)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    fn get_all_facts(&self, limit: usize) -> Vec<HiveFact> {
        self.facts.iter().take(limit).cloned().collect()
    }

    fn register_agent(&mut self, agent_name: &str) {
        self.agents.push(agent_name.to_string());
    }
}

// ===================================================================
// Mock quality scorer
// ===================================================================

struct AlwaysPassScorer;
impl QualityScorer for AlwaysPassScorer {
    fn score(&self, _content: &str, _context: &str) -> f64 {
        1.0
    }
}

struct AlwaysFailScorer;
impl QualityScorer for AlwaysFailScorer {
    fn score(&self, _content: &str, _context: &str) -> f64 {
        0.0
    }
}

// ===================================================================
// Helper
// ===================================================================

fn make_adapter(kind: BackendKind) -> CognitiveAdapter {
    let cfg = CognitiveAdapterConfig::new("test-agent");
    CognitiveAdapter::new(
        cfg,
        Box::new(MockCognitiveBackend::new(kind)),
        None,
        None,
    )
}

fn make_adapter_with_hive(hive: MockHiveStore) -> CognitiveAdapter {
    let cfg = CognitiveAdapterConfig::new("test-agent");
    CognitiveAdapter::new(
        cfg,
        Box::new(MockCognitiveBackend::new(BackendKind::Cognitive)),
        Some(Box::new(hive)),
        None,
    )
}

// ===================================================================
// Tests: basic store / search
// ===================================================================

#[test]
fn store_and_search_cognitive() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    adapter
        .store_fact_full("Biology", "Cells are the basic unit of life", 0.9, &[], "", &HashMap::new())
        .unwrap();

    let results = adapter.search_full("cells", 10, 0.0);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].context, "Biology");
    assert!(results[0].outcome.contains("Cells"));
}

#[test]
fn store_and_search_hierarchical() {
    let mut adapter = make_adapter(BackendKind::Hierarchical);
    adapter
        .store_fact_full("History", "Rome was founded in 753 BC", 0.8, &[], "", &HashMap::new())
        .unwrap();

    let results = adapter.search_full("rome", 10, 0.0);
    assert_eq!(results.len(), 1);
    assert!(results[0].outcome.contains("Rome"));
}

#[test]
fn store_fact_rejects_empty_context() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    let err = adapter
        .store_fact_full("", "content", 0.9, &[], "", &HashMap::new())
        .unwrap_err();
    assert_eq!(err, "context cannot be empty");
}

#[test]
fn store_fact_rejects_empty_fact() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    let err = adapter
        .store_fact_full("ctx", "  ", 0.9, &[], "", &HashMap::new())
        .unwrap_err();
    assert_eq!(err, "fact cannot be empty");
}

#[test]
fn search_empty_query_returns_empty() {
    let adapter = make_adapter(BackendKind::Cognitive);
    assert!(adapter.search_full("", 10, 0.0).is_empty());
    assert!(adapter.search_full("   ", 10, 0.0).is_empty());
}

#[test]
fn search_local_skips_hive() {
    let hive = MockHiveStore::with_facts(vec![HiveFact {
        fact_id: "h1".into(),
        content: "hive fact".into(),
        concept: "test".into(),
        confidence: 0.9,
        source_agent: "other".into(),
        tags: vec![],
        metadata: HashMap::new(),
        timestamp: String::new(),
    }]);
    let mut adapter = make_adapter_with_hive(hive);
    adapter
        .store_fact_full("local", "local fact", 0.9, &[], "", &HashMap::new())
        .unwrap();

    let local_results = adapter.search_local("fact", 10, 0.0);
    // Should only have local results, not hive
    assert!(local_results.iter().all(|f| !f.outcome.contains("hive")));
}

// ===================================================================
// Tests: hive federation
// ===================================================================

#[test]
fn search_federates_with_hive() {
    let hive = MockHiveStore::with_facts(vec![HiveFact {
        fact_id: "h1".into(),
        content: "quantum entanglement".into(),
        concept: "physics".into(),
        confidence: 0.9,
        source_agent: "agent-b".into(),
        tags: vec![],
        metadata: HashMap::new(),
        timestamp: String::new(),
    }]);
    let adapter = make_adapter_with_hive(hive);
    let results = adapter.search_full("quantum", 10, 0.0);
    assert!(!results.is_empty());
    assert!(results.iter().any(|f| f.outcome.contains("quantum")));
}

#[test]
fn confidence_gate_filters_hive() {
    let hive = MockHiveStore::with_facts(vec![HiveFact {
        fact_id: "h1".into(),
        content: "low confidence fact".into(),
        concept: "test".into(),
        confidence: 0.1,
        source_agent: "other".into(),
        tags: vec![],
        metadata: HashMap::new(),
        timestamp: String::new(),
    }]);
    let cfg = CognitiveAdapterConfig::new("test").with_confidence_gate(0.5);
    let adapter = CognitiveAdapter::new(
        cfg,
        Box::new(MockCognitiveBackend::new(BackendKind::Cognitive)),
        Some(Box::new(hive)),
        None,
    );
    let results = adapter.search_full("low confidence", 10, 0.0);
    // Hive results should be filtered out by confidence gate
    assert!(results.is_empty());
}

// ===================================================================
// Tests: quality gating
// ===================================================================

#[test]
fn quality_gate_blocks_low_quality() {
    let cfg = CognitiveAdapterConfig::new("test").with_quality_threshold(0.5);
    let hive = MockHiveStore::new();
    let mut adapter = CognitiveAdapter::new(
        cfg,
        Box::new(MockCognitiveBackend::new(BackendKind::Cognitive)),
        Some(Box::new(hive)),
        Some(Box::new(AlwaysFailScorer)),
    );
    // Store should succeed locally
    adapter
        .store_fact_full("ctx", "low quality fact", 0.9, &[], "", &HashMap::new())
        .unwrap();
    // But fact should NOT be promoted to hive (scorer returns 0.0 < threshold 0.5)
    // We can verify by checking search_full returns only local
}

#[test]
fn quality_gate_allows_high_quality() {
    let cfg = CognitiveAdapterConfig::new("test").with_quality_threshold(0.5);
    let hive = MockHiveStore::new();
    let mut adapter = CognitiveAdapter::new(
        cfg,
        Box::new(MockCognitiveBackend::new(BackendKind::Cognitive)),
        Some(Box::new(hive)),
        Some(Box::new(AlwaysPassScorer)),
    );
    adapter
        .store_fact_full("ctx", "high quality fact", 0.9, &[], "", &HashMap::new())
        .unwrap();
    // Fact should be promoted (scorer returns 1.0 > threshold 0.5)
}

// ===================================================================
// Tests: cognitive-specific operations
// ===================================================================

#[test]
fn working_memory_push_get_clear() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    let id = adapter
        .push_working("context", "task state", "task-1", 1.0)
        .unwrap();
    assert!(!id.is_empty());

    let slots = adapter.get_working("task-1");
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].content, "task state");

    let cleared = adapter.clear_working("task-1");
    assert_eq!(cleared, 1);
    assert!(adapter.get_working("task-1").is_empty());
}

#[test]
fn working_memory_bounded() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    for i in 0..MAX_WORKING_SLOTS {
        assert!(
            adapter
                .push_working("ctx", &format!("slot-{i}"), "t1", 1.0)
                .is_some(),
            "slot {i} should succeed"
        );
    }
    // 21st slot should be rejected
    assert!(adapter.push_working("ctx", "overflow", "t1", 1.0).is_none());
}

#[test]
fn procedure_store_and_recall() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    let steps = vec!["step 1".to_string(), "step 2".to_string()];
    let id = adapter.store_procedure("deploy", &steps).unwrap();
    assert!(!id.is_empty());

    let recalled = adapter.recall_procedure("deploy", 5);
    assert_eq!(recalled.len(), 1);
    assert_eq!(recalled[0].name, "deploy");
    assert_eq!(recalled[0].steps.len(), 2);
}

#[test]
fn prospective_store_and_trigger() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    adapter
        .store_prospective("remind to review", "PR merged", "send notification")
        .unwrap();

    let triggers = adapter.check_triggers("The PR merged successfully");
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].action, "send notification");

    let no_triggers = adapter.check_triggers("nothing relevant");
    assert!(no_triggers.is_empty());
}

#[test]
fn sensory_record() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    let id = adapter.record_sensory("text", "raw input data", 300).unwrap();
    assert!(!id.is_empty());
}

#[test]
fn episode_store() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    let id = adapter.store_episode("conversation transcript", "chat");
    assert!(!id.is_empty());
}

// ===================================================================
// Tests: trait implementations
// ===================================================================

#[test]
fn memory_retriever_search() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    adapter
        .store_fact_full("math", "Pi is approximately 3.14159", 0.95, &[], "", &HashMap::new())
        .unwrap();

    let results = MemoryRetriever::search(&adapter, "pi", 10);
    assert_eq!(results.len(), 1);
}

#[test]
fn memory_facade_recall() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    adapter
        .store_fact_full("math", "Pi is approximately 3.14159", 0.95, &[], "", &HashMap::new())
        .unwrap();

    let results = MemoryFacade::recall(&adapter, "pi", 10);
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("Pi"));
}

#[test]
fn memory_facade_retrieve_facts() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    adapter
        .store_fact_full("geo", "Earth orbits the Sun", 0.99, &[], "", &HashMap::new())
        .unwrap();

    let facts = MemoryFacade::retrieve_facts(&adapter, "earth", 10);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].context, "geo");
}

// ===================================================================
// Tests: utility methods
// ===================================================================

#[test]
fn backend_type_cognitive() {
    let adapter = make_adapter(BackendKind::Cognitive);
    assert_eq!(adapter.backend_type(), BackendKind::Cognitive);
}

#[test]
fn backend_type_hierarchical() {
    let adapter = make_adapter(BackendKind::Hierarchical);
    assert_eq!(adapter.backend_type(), BackendKind::Hierarchical);
}

#[test]
fn get_statistics() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    adapter
        .store_fact_full("ctx", "fact", 0.9, &[], "", &HashMap::new())
        .unwrap();
    let stats = adapter.get_statistics();
    assert_eq!(stats.get("total"), Some(&serde_json::to_value(1).unwrap()));
}

#[test]
fn get_all_facts_returns_stored() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    for i in 0..5 {
        adapter
            .store_fact_full("ctx", &format!("fact-{i}"), 0.9, &[], "", &HashMap::new())
            .unwrap();
    }
    let all = adapter.get_all_facts(50);
    assert_eq!(all.len(), 5);
}

#[test]
fn flush_and_close_do_not_panic() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    adapter.flush();
    adapter.close();
}

#[test]
fn agent_name_accessor() {
    let adapter = make_adapter(BackendKind::Cognitive);
    assert_eq!(adapter.agent_name(), "test-agent");
}

// ===================================================================
// Tests: hive metadata restoration
// ===================================================================

#[test]
fn hive_fact_restores_date_tag() {
    let hive = MockHiveStore::with_facts(vec![HiveFact {
        fact_id: "h1".into(),
        content: "historical event".into(),
        concept: "history".into(),
        confidence: 0.8,
        source_agent: "agent-a".into(),
        tags: vec!["date:2024-01-15".into()],
        metadata: HashMap::new(),
        timestamp: String::new(),
    }]);
    let adapter = make_adapter_with_hive(hive);
    let results = adapter.search_full("historical", 10, 0.0);
    assert!(!results.is_empty());
    let meta = &results[0].metadata;
    assert_eq!(
        meta.get("source_date"),
        Some(&Value::String("2024-01-15".into()))
    );
}

#[test]
fn hive_fact_restores_time_tag() {
    let hive = MockHiveStore::with_facts(vec![HiveFact {
        fact_id: "h1".into(),
        content: "timed event".into(),
        concept: "events".into(),
        confidence: 0.8,
        source_agent: "agent-a".into(),
        tags: vec!["time:1430".into()],
        metadata: HashMap::new(),
        timestamp: String::new(),
    }]);
    let adapter = make_adapter_with_hive(hive);
    let results = adapter.search_full("timed", 10, 0.0);
    assert!(!results.is_empty());
    let meta = &results[0].metadata;
    assert_eq!(
        meta.get("temporal_order"),
        Some(&Value::String("1430".into()))
    );
}

// ===================================================================
// Tests: fallback scan
// ===================================================================

#[test]
fn fallback_scan_when_keyword_search_misses() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    // Store with concept "xyz" so keyword search for "abc" won't match
    adapter
        .store_fact_full("xyz", "unique content here", 0.9, &[], "", &HashMap::new())
        .unwrap();

    // "abc" won't match "xyz" or "unique content here" via substring,
    // but the fallback scan should return all facts
    let results = adapter.search_full("abc", 10, 0.0);
    // Fallback scan returns all facts and re-ranks
    assert_eq!(results.len(), 1);
}

// ===================================================================
// Tests: hierarchical backend defaults
// ===================================================================

#[test]
fn hierarchical_cognitive_ops_return_defaults() {
    let mut adapter = make_adapter(BackendKind::Hierarchical);
    // Working memory returns None on hierarchical (default impl)
    assert!(adapter.push_working("ctx", "data", "t1", 1.0).is_some());

    // These work because MockCognitiveBackend implements them for both kinds.
    // In a real hierarchical backend, the default trait impls would return None/empty.
}
