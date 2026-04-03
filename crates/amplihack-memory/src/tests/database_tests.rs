use super::*;
use crate::database_helpers::{iso_to_epoch, str_to_memory_type};
use crate::models::{MemoryEntry, MemoryType};

#[test]
fn content_hash_deterministic() {
    let h1 = content_hash("hello world");
    let h2 = content_hash("hello world");
    assert_eq!(h1, h2);
    assert_ne!(h1, content_hash("goodbye"));
}

#[test]
fn iso_round_trip() {
    let iso = "2024-06-15T12:30:45Z";
    let epoch = iso_to_epoch(iso);
    assert!(epoch > 1_718_000_000.0);
}

#[test]
fn str_to_memory_type_all_variants() {
    assert_eq!(str_to_memory_type("episodic"), Some(MemoryType::Episodic));
    assert_eq!(str_to_memory_type("semantic"), Some(MemoryType::Semantic));
    assert_eq!(str_to_memory_type("working"), Some(MemoryType::Working));
    assert_eq!(str_to_memory_type("task"), Some(MemoryType::Task));
    assert_eq!(str_to_memory_type("bogus"), None);
}

#[cfg(feature = "sqlite")]
#[test]
fn db_store_and_retrieve() {
    let db = MemoryDatabase::open_in_memory().unwrap();
    let entry = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "test content");
    assert!(db.store_memory(&entry).unwrap());

    let query = DbQuery {
        session_id: Some("s1".into()),
        ..Default::default()
    };
    let results = db.retrieve_memories(&query).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "test content");
}

#[cfg(feature = "sqlite")]
#[test]
fn db_delete_memory() {
    let db = MemoryDatabase::open_in_memory().unwrap();
    let entry = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "delete me");
    db.store_memory(&entry).unwrap();
    let id = entry.id.clone();
    assert!(db.delete_memory(&id).unwrap());
    assert!(!db.delete_memory(&id).unwrap());
}

#[cfg(feature = "sqlite")]
#[test]
fn db_session_tracking() {
    let db = MemoryDatabase::open_in_memory().unwrap();
    let e1 = MemoryEntry::new("s1", "agent-a", MemoryType::Semantic, "fact 1");
    let e2 = MemoryEntry::new("s1", "agent-b", MemoryType::Episodic, "event 1");
    db.store_memory(&e1).unwrap();
    db.store_memory(&e2).unwrap();

    let info = db.get_session_info("s1").unwrap().unwrap();
    assert_eq!(info.memory_count, 2);
    assert_eq!(info.agent_ids.len(), 2);
}

#[cfg(feature = "sqlite")]
#[test]
fn db_list_sessions() {
    let db = MemoryDatabase::open_in_memory().unwrap();
    let e1 = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "c1");
    let e2 = MemoryEntry::new("s2", "a1", MemoryType::Working, "c2");
    db.store_memory(&e1).unwrap();
    db.store_memory(&e2).unwrap();

    let sessions = db.list_sessions(None).unwrap();
    assert_eq!(sessions.len(), 2);
}

#[cfg(feature = "sqlite")]
#[test]
fn db_delete_session() {
    let db = MemoryDatabase::open_in_memory().unwrap();
    let e = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "content");
    db.store_memory(&e).unwrap();
    assert!(db.delete_session("s1").unwrap());
    assert!(!db.delete_session("s1").unwrap());
}

#[cfg(feature = "sqlite")]
#[test]
fn db_stats() {
    let db = MemoryDatabase::open_in_memory().unwrap();
    let e = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "content");
    db.store_memory(&e).unwrap();
    let stats = db.get_stats().unwrap();
    assert_eq!(stats.total_memories, 1);
    assert_eq!(stats.total_sessions, 1);
}

#[cfg(feature = "sqlite")]
#[test]
fn db_get_by_id() {
    let db = MemoryDatabase::open_in_memory().unwrap();
    let entry = MemoryEntry::new("s1", "a1", MemoryType::Procedural, "how-to");
    let id = entry.id.clone();
    db.store_memory(&entry).unwrap();
    let found = db.get_by_id(&id).unwrap().unwrap();
    assert_eq!(found.content, "how-to");
    assert!(db.get_by_id("nonexistent").unwrap().is_none());
}

#[cfg(feature = "sqlite")]
#[test]
fn db_content_search() {
    let db = MemoryDatabase::open_in_memory().unwrap();
    let e1 = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "the quick brown fox");
    let e2 = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "lazy dog");
    db.store_memory(&e1).unwrap();
    db.store_memory(&e2).unwrap();

    let query = DbQuery {
        content_search: Some("fox".into()),
        ..Default::default()
    };
    let results = db.retrieve_memories(&query).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("fox"));
}

#[cfg(feature = "sqlite")]
#[test]
fn db_vacuum_and_optimize() {
    let db = MemoryDatabase::open_in_memory().unwrap();
    assert!(db.vacuum().is_ok());
    assert!(db.optimize().is_ok());
}
