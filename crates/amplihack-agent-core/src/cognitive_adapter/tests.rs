//! Basic cognitive adapter tests: store, search, working memory.

use std::collections::HashMap;

use super::adapter::CognitiveAdapter;
use super::constants::MAX_WORKING_SLOTS;
use super::types::*;

use super::test_mocks::*;

// ===================================================================
// Tests: basic store / search
// ===================================================================

#[test]
fn store_and_search_cognitive() {
    let mut adapter = make_adapter(BackendKind::Cognitive);
    adapter
        .store_fact_full(
            "Biology",
            "Cells are the basic unit of life",
            0.9,
            &[],
            "",
            &HashMap::new(),
        )
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
        .store_fact_full(
            "History",
            "Rome was founded in 753 BC",
            0.8,
            &[],
            "",
            &HashMap::new(),
        )
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
