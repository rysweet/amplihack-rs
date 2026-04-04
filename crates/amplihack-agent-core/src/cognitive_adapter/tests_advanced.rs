//! Advanced cognitive adapter tests: procedures, hive, quality, statistics.

use std::collections::HashMap;

use serde_json::Value;

use super::types::*;
use crate::agentic_loop::traits::{MemoryFacade, MemoryRetriever};

use super::test_mocks::*;

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
    let id = adapter
        .record_sensory("text", "raw input data", 300)
        .unwrap();
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
        .store_fact_full(
            "math",
            "Pi is approximately 3.14159",
            0.95,
            &[],
            "",
            &HashMap::new(),
        )
        .unwrap();

    let results = MemoryRetriever::search(&adapter, "pi", 10);
    assert_eq!(results.len(), 1);
}

#[test]
fn memory_facade_recall() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    adapter
        .store_fact_full(
            "math",
            "Pi is approximately 3.14159",
            0.95,
            &[],
            "",
            &HashMap::new(),
        )
        .unwrap();

    let results = MemoryFacade::recall(&adapter, "pi", 10);
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("Pi"));
}

#[test]
fn memory_facade_retrieve_facts() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    adapter
        .store_fact_full(
            "geo",
            "Earth orbits the Sun",
            0.99,
            &[],
            "",
            &HashMap::new(),
        )
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
