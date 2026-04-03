//! Tests for MemoryFacade.

use amplihack_memory::backend::InMemoryBackend;
use amplihack_memory::config::MemoryConfig;
use amplihack_memory::facade::{MemoryFacade, RecallOptions, StoreOptions};
use amplihack_memory::models::MemoryType;

fn test_facade() -> MemoryFacade {
    let backend = Box::new(InMemoryBackend::new());
    MemoryFacade::new(backend, MemoryConfig::for_testing())
}

// ── Construction ──

#[test]
fn facade_new_with_in_memory_backend() {
    let facade = test_facade();
    assert_eq!(facade.backend_name(), "in_memory");
}

#[test]
fn facade_reports_backend_name() {
    let facade = test_facade();
    assert_eq!(facade.backend_name(), "in_memory");
}

#[test]
fn facade_config_accessible() {
    let facade = test_facade();
    let config = facade.config();
    assert!(!config.quality_review_enabled);
}

#[test]
fn facade_auto_creates_facade() {
    let facade = MemoryFacade::auto(MemoryConfig::for_testing()).unwrap();
    // With testing config (InMemory backend), auto should succeed
    assert!(!facade.backend_name().is_empty());
}

// ── store_memory ──

#[test]
fn facade_store_memory_returns_id() {
    let mut facade = test_facade();
    let opts = StoreOptions::new(MemoryType::Semantic, "test-session");
    let id = facade
        .store_memory("Test content for facade store", opts)
        .unwrap();
    assert!(!id.is_empty());
}

#[test]
fn facade_store_memory_with_importance() {
    let mut facade = test_facade();
    let mut opts = StoreOptions::new(MemoryType::Procedural, "s1");
    opts.importance = Some(0.9);
    let id = facade
        .store_memory("High importance content here", opts)
        .unwrap();
    assert!(!id.is_empty());
}

#[test]
fn facade_store_memory_with_tags() {
    let mut facade = test_facade();
    let mut opts = StoreOptions::new(MemoryType::Semantic, "s1");
    opts.tags = vec!["rust".to_string(), "testing".to_string()];
    let id = facade
        .store_memory("Tagged content for test", opts)
        .unwrap();
    assert!(!id.is_empty());
}

// ── recall ──

#[test]
fn facade_recall_returns_stored() {
    let mut facade = test_facade();
    let opts = StoreOptions::new(MemoryType::Semantic, "s1");
    facade
        .store_memory("Rust is a systems programming language", opts)
        .unwrap();
    let results = facade.recall("Rust", RecallOptions::default()).unwrap();
    assert!(!results.is_empty());
}

#[test]
fn facade_recall_with_session_filter() {
    let mut facade = test_facade();
    facade
        .store_memory(
            "Session one content here",
            StoreOptions::new(MemoryType::Semantic, "s1"),
        )
        .unwrap();
    facade
        .store_memory(
            "Session two content here",
            StoreOptions::new(MemoryType::Semantic, "s2"),
        )
        .unwrap();
    let opts = RecallOptions {
        session_id: Some("s1".to_string()),
        ..RecallOptions::default()
    };
    let results = facade.recall("content", opts).unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn facade_recall_with_type_filter() {
    let mut facade = test_facade();
    facade
        .store_memory(
            "Semantic knowledge content here",
            StoreOptions::new(MemoryType::Semantic, "s1"),
        )
        .unwrap();
    facade
        .store_memory(
            "How to do procedural things",
            StoreOptions::new(MemoryType::Procedural, "s1"),
        )
        .unwrap();
    let opts = RecallOptions {
        memory_types: vec![MemoryType::Procedural],
        ..RecallOptions::default()
    };
    let results = facade.recall("how to", opts).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory_type, MemoryType::Procedural);
}

#[test]
fn facade_recall_empty_when_nothing_stored() {
    let facade = test_facade();
    let opts = RecallOptions {
        token_budget: 500,
        ..RecallOptions::default()
    };
    let results = facade.recall("short query", opts).unwrap();
    assert!(results.is_empty());
}

// ── forget ──

#[test]
fn facade_forget_returns_true_when_existed() {
    let mut facade = test_facade();
    let opts = StoreOptions::new(MemoryType::Semantic, "s1");
    let id = facade
        .store_memory("Content to forget later", opts)
        .unwrap();
    assert!(facade.forget(&id).unwrap());
    // Second forget should return false
    assert!(!facade.forget(&id).unwrap());
}

#[test]
fn facade_forget_returns_false_for_missing() {
    let mut facade = test_facade();
    assert!(!facade.forget("nonexistent-id").unwrap());
}

// ── list_sessions ──

#[test]
fn facade_list_sessions_shows_stored() {
    let mut facade = test_facade();
    facade
        .store_memory(
            "Session content for listing test",
            StoreOptions::new(MemoryType::Semantic, "session-a"),
        )
        .unwrap();
    let sessions = facade.list_sessions().unwrap();
    assert!(!sessions.is_empty());
    assert!(sessions.iter().any(|s| s.session_id == "session-a"));
}

#[test]
fn facade_list_sessions_empty_initially() {
    let facade = test_facade();
    let sessions = facade.list_sessions().unwrap();
    assert!(sessions.is_empty());
}

// ── StoreOptions / RecallOptions ──

#[test]
fn store_options_defaults() {
    let opts = StoreOptions::default();
    assert_eq!(opts.memory_type, MemoryType::Semantic);
    assert_eq!(opts.session_id, "default");
    assert_eq!(opts.agent_id, "default");
    assert!(opts.importance.is_none());
    assert!(opts.tags.is_empty());
}

#[test]
fn store_options_new() {
    let opts = StoreOptions::new(MemoryType::Working, "my-session");
    assert_eq!(opts.memory_type, MemoryType::Working);
    assert_eq!(opts.session_id, "my-session");
}

#[test]
fn recall_options_defaults() {
    let opts = RecallOptions::default();
    assert!(opts.session_id.is_none());
    assert!(opts.memory_types.is_empty());
    assert_eq!(opts.token_budget, 4000);
    assert_eq!(opts.limit, 20);
}
