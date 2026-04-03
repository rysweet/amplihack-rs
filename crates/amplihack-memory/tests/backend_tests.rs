//! Comprehensive tests for MemoryBackend trait and SqliteBackend.

use amplihack_memory::backend::{BackendHealth, InMemoryBackend, MemoryBackend};
use amplihack_memory::models::{MemoryEntry, MemoryQuery, MemoryType};

fn make_entry(session: &str, content: &str, mem_type: MemoryType) -> MemoryEntry {
    MemoryEntry::new(session, "agent-1", mem_type, content)
}

fn semantic_entry(content: &str) -> MemoryEntry {
    make_entry("test-session", content, MemoryType::Semantic)
}

// ── InMemoryBackend basic tests ──

#[test]
fn in_memory_store_returns_id() {
    let mut b = InMemoryBackend::new();
    let id = b.store(&semantic_entry("Store test content")).unwrap();
    assert!(!id.is_empty());
}

#[test]
fn in_memory_retrieve_by_text() {
    let mut b = InMemoryBackend::new();
    b.store(&semantic_entry("The sky is blue and vast"))
        .unwrap();
    b.store(&semantic_entry("Grass grows green in spring"))
        .unwrap();
    let results = b.retrieve(&MemoryQuery::new("sky")).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("sky"));
}

#[test]
fn in_memory_retrieve_empty_query_returns_all() {
    let mut b = InMemoryBackend::new();
    b.store(&semantic_entry("First entry content here"))
        .unwrap();
    b.store(&semantic_entry("Second entry content here"))
        .unwrap();
    let results = b.retrieve(&MemoryQuery::new("")).unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn in_memory_retrieve_by_session() {
    let mut b = InMemoryBackend::new();
    b.store(&make_entry(
        "s1",
        "Entry for session one",
        MemoryType::Semantic,
    ))
    .unwrap();
    b.store(&make_entry(
        "s2",
        "Entry for session two",
        MemoryType::Semantic,
    ))
    .unwrap();
    let q = MemoryQuery::new("").with_session("s1");
    let results = b.retrieve(&q).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].session_id, "s1");
}

#[test]
fn in_memory_retrieve_by_memory_type() {
    let mut b = InMemoryBackend::new();
    b.store(&make_entry(
        "s1",
        "Semantic knowledge here",
        MemoryType::Semantic,
    ))
    .unwrap();
    b.store(&make_entry(
        "s1",
        "Working context here",
        MemoryType::Working,
    ))
    .unwrap();
    b.store(&make_entry(
        "s1",
        "Procedural steps here",
        MemoryType::Procedural,
    ))
    .unwrap();
    let q = MemoryQuery::new("").with_types(vec![MemoryType::Working]);
    let results = b.retrieve(&q).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory_type, MemoryType::Working);
}

#[test]
fn in_memory_delete_existing_entry() {
    let mut b = InMemoryBackend::new();
    let entry = semantic_entry("Content to delete eventually");
    let id = b.store(&entry).unwrap();
    assert!(b.delete(&id).unwrap());
    let results = b.retrieve(&MemoryQuery::new("")).unwrap();
    assert!(results.is_empty());
}

#[test]
fn in_memory_delete_nonexistent_returns_false() {
    let mut b = InMemoryBackend::new();
    assert!(!b.delete("nonexistent-id").unwrap());
}

#[test]
fn in_memory_list_sessions() {
    let mut b = InMemoryBackend::new();
    b.store(&make_entry(
        "s1",
        "First session entry here",
        MemoryType::Semantic,
    ))
    .unwrap();
    b.store(&make_entry(
        "s1",
        "Another first session entry",
        MemoryType::Working,
    ))
    .unwrap();
    b.store(&make_entry(
        "s2",
        "Second session entry here",
        MemoryType::Semantic,
    ))
    .unwrap();
    let sessions = b.list_sessions().unwrap();
    assert_eq!(sessions.len(), 2);
    let s1 = sessions.iter().find(|s| s.session_id == "s1").unwrap();
    assert_eq!(s1.memory_count, 2);
}

#[test]
fn in_memory_health_check_always_healthy() {
    let b = InMemoryBackend::new();
    let health = b.health_check().unwrap();
    assert!(health.healthy);
    assert_eq!(health.entry_count, 0);
}

#[test]
fn in_memory_health_check_counts_entries() {
    let mut b = InMemoryBackend::new();
    b.store(&semantic_entry("Some entry for health check"))
        .unwrap();
    b.store(&semantic_entry("Another entry for counting"))
        .unwrap();
    let health = b.health_check().unwrap();
    assert_eq!(health.entry_count, 2);
}

#[test]
fn in_memory_backend_name() {
    let b = InMemoryBackend::new();
    assert_eq!(b.backend_name(), "in_memory");
}

#[test]
fn in_memory_retrieve_respects_limit() {
    let mut b = InMemoryBackend::new();
    for i in 0..50 {
        b.store(&semantic_entry(&format!("Entry number {i} with content")))
            .unwrap();
    }
    let mut q = MemoryQuery::new("");
    q.limit = 5;
    let results = b.retrieve(&q).unwrap();
    assert_eq!(results.len(), 5);
}

// ── All MemoryType variants ──

#[test]
fn store_all_cognitive_types() {
    let mut b = InMemoryBackend::new();
    for (mt, content) in [
        (MemoryType::Episodic, "Episodic event happened here"),
        (MemoryType::Procedural, "Step 1: do this. Step 2: do that."),
        (MemoryType::Prospective, "Remember to deploy on Friday"),
        (MemoryType::Working, "Current task context for agent"),
        (MemoryType::Strategic, "Long-term strategic goal content"),
    ] {
        let id = b.store(&make_entry("s1", content, mt)).unwrap();
        assert!(!id.is_empty());
        let q = MemoryQuery::new("").with_types(vec![mt]);
        assert_eq!(b.retrieve(&q).unwrap().len(), 1);
    }
}

#[test]
fn store_legacy_types() {
    let mut b = InMemoryBackend::new();
    b.store(&make_entry(
        "s1",
        "Code context for legacy path",
        MemoryType::CodeContext,
    ))
    .unwrap();
    b.store(&make_entry(
        "s1",
        "Task-specific memory content here",
        MemoryType::Task,
    ))
    .unwrap();
    let q = MemoryQuery::new("").with_types(vec![MemoryType::CodeContext]);
    assert_eq!(b.retrieve(&q).unwrap().len(), 1);
    let q = MemoryQuery::new("").with_types(vec![MemoryType::Task]);
    assert_eq!(b.retrieve(&q).unwrap().len(), 1);
}

#[test]
fn retrieve_multiple_types() {
    let mut b = InMemoryBackend::new();
    b.store(&make_entry(
        "s1",
        "Semantic knowledge content here",
        MemoryType::Semantic,
    ))
    .unwrap();
    b.store(&make_entry(
        "s1",
        "Working memory context here",
        MemoryType::Working,
    ))
    .unwrap();
    b.store(&make_entry(
        "s1",
        "Episodic event record here",
        MemoryType::Episodic,
    ))
    .unwrap();
    let q = MemoryQuery::new("").with_types(vec![MemoryType::Semantic, MemoryType::Episodic]);
    let results = b.retrieve(&q).unwrap();
    assert_eq!(results.len(), 2);
}

// ── Duplicate detection ──

#[test]
fn store_duplicate_content_creates_two_entries() {
    let mut b = InMemoryBackend::new();
    let content = "Exact same content stored twice";
    b.store(&semantic_entry(content)).unwrap();
    b.store(&semantic_entry(content)).unwrap();
    // InMemoryBackend doesn't dedup — that's the coordinator's job
    let results = b.retrieve(&MemoryQuery::new("")).unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn entries_have_unique_ids() {
    let mut b = InMemoryBackend::new();
    let id1 = b
        .store(&semantic_entry("First unique entry content"))
        .unwrap();
    let id2 = b
        .store(&semantic_entry("Second unique entry content"))
        .unwrap();
    assert_ne!(id1, id2);
}

// ── Large content ──

#[test]
fn store_large_content() {
    let mut b = InMemoryBackend::new();
    let content = "x".repeat(50_000);
    let id = b.store(&semantic_entry(&content)).unwrap();
    assert!(!id.is_empty());
    let results = b.retrieve(&MemoryQuery::new("")).unwrap();
    assert_eq!(results[0].content.len(), 50_000);
}

#[test]
fn store_empty_content() {
    let mut b = InMemoryBackend::new();
    let id = b.store(&semantic_entry("")).unwrap();
    assert!(!id.is_empty());
}

// ── Session listing ──

#[test]
fn list_sessions_empty_backend() {
    let b = InMemoryBackend::new();
    let sessions = b.list_sessions().unwrap();
    assert!(sessions.is_empty());
}

#[test]
fn list_sessions_tracks_agent_ids() {
    let mut b = InMemoryBackend::new();
    let mut e1 = semantic_entry("Entry from agent one here");
    e1.agent_id = "agent-1".to_string();
    e1.session_id = "s1".to_string();
    b.store(&e1).unwrap();
    let mut e2 = semantic_entry("Entry from agent two here");
    e2.agent_id = "agent-2".to_string();
    e2.session_id = "s1".to_string();
    b.store(&e2).unwrap();
    let sessions = b.list_sessions().unwrap();
    let s = sessions.iter().find(|s| s.session_id == "s1").unwrap();
    assert_eq!(s.agent_ids.len(), 2);
}

// ── BackendHealth struct ──

#[test]
fn backend_health_ok_constructor() {
    let h = BackendHealth::ok("test");
    assert!(h.healthy);
    assert_eq!(h.backend_name, "test");
    assert_eq!(h.latency_ms, 0.0);
}

#[test]
fn backend_health_degraded_constructor() {
    let h = BackendHealth::degraded("test", "disk full");
    assert!(!h.healthy);
    assert_eq!(h.details, "disk full");
}

#[test]
fn backend_health_serializes() {
    let h = BackendHealth::ok("sqlite");
    let json = serde_json::to_value(&h).unwrap();
    assert_eq!(json["healthy"], true);
    assert_eq!(json["backend_name"], "sqlite");
}

// ── SQLite backend tests (compile check + feature-gated) ──

#[cfg(feature = "sqlite")]
mod sqlite_tests {
    use amplihack_memory::backend::MemoryBackend;
    use amplihack_memory::models::{MemoryEntry, MemoryQuery, MemoryType};
    use amplihack_memory::sqlite_backend::SqliteBackend;

    fn make_entry(content: &str) -> MemoryEntry {
        MemoryEntry::new("test-session", "agent-1", MemoryType::Semantic, content)
    }

    #[test]
    fn sqlite_open_in_memory() {
        let backend = SqliteBackend::open_in_memory().unwrap();
        assert_eq!(backend.backend_name(), "sqlite");
    }

    #[test]
    fn sqlite_wal_mode_enabled() {
        // In-memory databases report journal_mode=memory, not wal.
        // Test with a file-backed database.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("wal_test.db");
        let backend = SqliteBackend::open(&db_path).unwrap();
        assert!(backend.is_wal_mode().unwrap());
    }

    #[test]
    fn sqlite_store_and_retrieve() {
        let mut backend = SqliteBackend::open_in_memory().unwrap();
        let entry = make_entry("SQLite test content for retrieval");
        let id = backend.store(&entry).unwrap();
        assert!(!id.is_empty());
        let results = backend.retrieve(&MemoryQuery::new("")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "SQLite test content for retrieval");
    }

    #[test]
    fn sqlite_store_multiple_types() {
        let mut backend = SqliteBackend::open_in_memory().unwrap();
        backend
            .store(&MemoryEntry::new(
                "s1",
                "a1",
                MemoryType::Semantic,
                "Semantic data",
            ))
            .unwrap();
        backend
            .store(&MemoryEntry::new(
                "s1",
                "a1",
                MemoryType::Episodic,
                "Episodic data",
            ))
            .unwrap();
        backend
            .store(&MemoryEntry::new(
                "s1",
                "a1",
                MemoryType::Working,
                "Working data",
            ))
            .unwrap();
        let all = backend.retrieve(&MemoryQuery::new("")).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn sqlite_retrieve_by_type() {
        let mut backend = SqliteBackend::open_in_memory().unwrap();
        backend
            .store(&MemoryEntry::new(
                "s1",
                "a1",
                MemoryType::Semantic,
                "Semantic only",
            ))
            .unwrap();
        backend
            .store(&MemoryEntry::new(
                "s1",
                "a1",
                MemoryType::Working,
                "Working only",
            ))
            .unwrap();
        let q = MemoryQuery::new("").with_types(vec![MemoryType::Working]);
        let results = backend.retrieve(&q).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_type, MemoryType::Working);
    }

    #[test]
    fn sqlite_retrieve_by_session() {
        let mut backend = SqliteBackend::open_in_memory().unwrap();
        backend
            .store(&MemoryEntry::new(
                "s1",
                "a1",
                MemoryType::Semantic,
                "Session one data",
            ))
            .unwrap();
        backend
            .store(&MemoryEntry::new(
                "s2",
                "a1",
                MemoryType::Semantic,
                "Session two data",
            ))
            .unwrap();
        let q = MemoryQuery::new("").with_session("s1");
        let results = backend.retrieve(&q).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "s1");
    }

    #[test]
    fn sqlite_delete() {
        let mut backend = SqliteBackend::open_in_memory().unwrap();
        let entry = make_entry("Content to be deleted from sqlite");
        let id = backend.store(&entry).unwrap();
        assert!(backend.delete(&id).unwrap());
        assert!(!backend.delete(&id).unwrap());
    }

    #[test]
    fn sqlite_full_text_search() {
        let mut backend = SqliteBackend::open_in_memory().unwrap();
        backend
            .store(&make_entry("The quick brown fox jumps over"))
            .unwrap();
        backend
            .store(&make_entry("A lazy dog sleeps in the sun"))
            .unwrap();
        let results = backend.full_text_search("quick fox", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("quick"));
    }

    #[test]
    fn sqlite_list_sessions() {
        let mut backend = SqliteBackend::open_in_memory().unwrap();
        backend
            .store(&MemoryEntry::new(
                "s1",
                "a1",
                MemoryType::Semantic,
                "Session one",
            ))
            .unwrap();
        backend
            .store(&MemoryEntry::new(
                "s2",
                "a1",
                MemoryType::Semantic,
                "Session two",
            ))
            .unwrap();
        let sessions = backend.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn sqlite_health_check() {
        let backend = SqliteBackend::open_in_memory().unwrap();
        let health = backend.health_check().unwrap();
        assert!(health.healthy);
        assert_eq!(health.backend_name, "sqlite");
    }

    #[test]
    fn sqlite_large_content() {
        let mut backend = SqliteBackend::open_in_memory().unwrap();
        let content = "y".repeat(100_000);
        backend.store(&make_entry(&content)).unwrap();
        let results = backend.retrieve(&MemoryQuery::new("")).unwrap();
        assert_eq!(results[0].content.len(), 100_000);
    }

    #[test]
    fn sqlite_open_with_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let backend = SqliteBackend::open(&db_path).unwrap();
        assert_eq!(backend.backend_name(), "sqlite");
    }

    #[test]
    fn sqlite_concurrent_reads() {
        let mut backend = SqliteBackend::open_in_memory().unwrap();
        for i in 0..100 {
            backend
                .store(&make_entry(&format!("Concurrent entry {i}")))
                .unwrap();
        }
        // Multiple retrieve calls should not conflict
        let r1 = backend.retrieve(&MemoryQuery::new("")).unwrap();
        let r2 = backend.retrieve(&MemoryQuery::new("")).unwrap();
        assert_eq!(r1.len(), r2.len());
    }
}
