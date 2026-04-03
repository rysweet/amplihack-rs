use super::*;
use crate::backend::InMemoryBackend;

fn make_manager() -> MemoryManager {
    MemoryManager::new(Box::new(InMemoryBackend::new()), "test-session")
}

#[test]
fn store_and_retrieve() {
    let mut mgr = make_manager();
    let req = StoreRequest::new("a1", "Test", "hello world", MemoryType::Semantic);
    let id = mgr.store(req).unwrap();
    assert!(!id.is_empty());

    let results = mgr.retrieve(RetrieveCriteria::default()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "hello world");
}

#[test]
fn get_by_id() {
    let mut mgr = make_manager();
    let req = StoreRequest::new("a1", "X", "content", MemoryType::Episodic);
    let id = mgr.store(req).unwrap();
    let entry = mgr.get(&id).unwrap().unwrap();
    assert_eq!(entry.content, "content");
    assert!(mgr.get("nonexistent").unwrap().is_none());
}

#[test]
fn update_entry() {
    let mut mgr = make_manager();
    let req = StoreRequest::new("a1", "Old", "old content", MemoryType::Semantic);
    let id = mgr.store(req).unwrap();

    let upd = UpdateRequest {
        title: Some("New".into()),
        content: Some("new content".into()),
        ..Default::default()
    };
    assert!(mgr.update(&id, upd).unwrap());

    let entry = mgr.get(&id).unwrap().unwrap();
    assert_eq!(entry.title, "New");
    assert_eq!(entry.content, "new content");
}

#[test]
fn delete_entry() {
    let mut mgr = make_manager();
    let req = StoreRequest::new("a1", "Del", "delete me", MemoryType::Working);
    let id = mgr.store(req).unwrap();
    assert!(mgr.delete(&id).unwrap());
    assert!(mgr.get(&id).unwrap().is_none());
}

#[test]
fn store_batch() {
    let mut mgr = make_manager();
    let requests = vec![
        StoreRequest::new("a1", "T1", "c1", MemoryType::Semantic),
        StoreRequest::new("a1", "T2", "c2", MemoryType::Episodic),
    ];
    let ids = mgr.store_batch(requests);
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| id.is_some()));
}

#[test]
fn search() {
    let mut mgr = make_manager();
    mgr.store(StoreRequest::new(
        "a1",
        "Fox",
        "the quick brown fox",
        MemoryType::Semantic,
    ))
    .unwrap();
    mgr.store(StoreRequest::new(
        "a1",
        "Dog",
        "lazy dog",
        MemoryType::Semantic,
    ))
    .unwrap();

    let results = mgr.search("fox", None, 10).unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn get_important() {
    let mut mgr = make_manager();
    let mut req = StoreRequest::new("a1", "Hi", "important", MemoryType::Semantic);
    req.importance = Some(0.9);
    mgr.store(req).unwrap();

    let mut req2 = StoreRequest::new("a1", "Lo", "trivial", MemoryType::Semantic);
    req2.importance = Some(0.2);
    mgr.store(req2).unwrap();

    let results = mgr.get_important(0.7, 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "important");
}

#[test]
fn auto_session_id_format() {
    let id = generate_session_id();
    assert!(id.starts_with("session_"));
    assert!(id.len() > 16);
}

#[test]
fn list_memory_types() {
    let types = MemoryManager::list_memory_types();
    assert_eq!(types.len(), 12);
    assert!(types.contains(&"episodic"));
    assert!(types.contains(&"task"));
}

#[test]
fn backend_name() {
    let mgr = make_manager();
    assert_eq!(mgr.backend_name(), "in_memory");
}

#[test]
fn update_nonexistent_returns_false() {
    let mut mgr = make_manager();
    let upd = UpdateRequest {
        title: Some("New".into()),
        ..Default::default()
    };
    assert!(!mgr.update("no-such-id", upd).unwrap());
}

#[test]
fn delete_nonexistent_returns_false() {
    let mut mgr = make_manager();
    assert!(!mgr.delete("no-such-id").unwrap());
}
