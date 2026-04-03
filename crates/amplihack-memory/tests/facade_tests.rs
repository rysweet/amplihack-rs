//! Tests for MemoryFacade.
//!
//! Tests compile but FAIL because facade methods use todo!().

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
#[should_panic(expected = "not yet implemented")]
fn facade_auto_not_implemented() {
    let _ = MemoryFacade::auto(MemoryConfig::for_testing());
}

// ── store_memory ──

#[test]
#[should_panic(expected = "not yet implemented")]
fn facade_store_memory_not_implemented() {
    let mut facade = test_facade();
    let opts = StoreOptions::new(MemoryType::Semantic, "test-session");
    let _ = facade.store_memory("Test content for facade store", opts);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn facade_store_memory_with_importance() {
    let mut facade = test_facade();
    let mut opts = StoreOptions::new(MemoryType::Procedural, "s1");
    opts.importance = Some(0.9);
    let _ = facade.store_memory("High importance content here", opts);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn facade_store_memory_with_tags() {
    let mut facade = test_facade();
    let mut opts = StoreOptions::new(MemoryType::Semantic, "s1");
    opts.tags = vec!["rust".to_string(), "testing".to_string()];
    let _ = facade.store_memory("Tagged content for test", opts);
}

// ── recall ──

#[test]
#[should_panic(expected = "not yet implemented")]
fn facade_recall_not_implemented() {
    let facade = test_facade();
    let _ = facade.recall("test query", RecallOptions::default());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn facade_recall_with_session() {
    let facade = test_facade();
    let opts = RecallOptions {
        session_id: Some("s1".to_string()),
        ..RecallOptions::default()
    };
    let _ = facade.recall("query text", opts);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn facade_recall_with_type_filter() {
    let facade = test_facade();
    let opts = RecallOptions {
        memory_types: vec![MemoryType::Procedural],
        ..RecallOptions::default()
    };
    let _ = facade.recall("how to", opts);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn facade_recall_with_budget() {
    let facade = test_facade();
    let opts = RecallOptions {
        token_budget: 500,
        ..RecallOptions::default()
    };
    let _ = facade.recall("short query", opts);
}

// ── forget ──

#[test]
#[should_panic(expected = "not yet implemented")]
fn facade_forget_not_implemented() {
    let mut facade = test_facade();
    let _ = facade.forget("entry-id-1");
}

// ── list_sessions ──

#[test]
#[should_panic(expected = "not yet implemented")]
fn facade_list_sessions_not_implemented() {
    let facade = test_facade();
    let _ = facade.list_sessions();
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
