// crates/amplihack-memory/tests/agent_memory.rs
//
// TDD: failing tests for the AgentMemory adapter that ports
// amplifier-bundle/tools/amplihack/memory/interface.py to native Rust.
//
// Contract: AgentMemory is a thin adapter over MemoryManager exposing a
// stable, minimal session-scoped key/value API for agents.

use amplihack_memory::agent_memory::{AgentMemory, MemoryType};
use tempfile::TempDir;

fn temp_memory(agent: &str) -> (AgentMemory, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("memory.db");
    let mem = AgentMemory::builder()
        .agent_name(agent)
        .db_path(&db)
        .build()
        .expect("AgentMemory::build");
    (mem, dir)
}

#[test]
fn new_memory_auto_generates_session_id() {
    let (mem, _d) = temp_memory("agent-a");
    let sid = mem.session_id();
    assert!(
        sid.starts_with("agent-a_"),
        "session id should be prefixed with agent name, got {sid}"
    );
    assert!(sid.len() > "agent-a_".len() + 8);
}

#[test]
fn explicit_session_id_is_honored() {
    let dir = TempDir::new().unwrap();
    let mem = AgentMemory::builder()
        .agent_name("agent-b")
        .session_id("custom-session-123")
        .db_path(dir.path().join("memory.db"))
        .build()
        .unwrap();
    assert_eq!(mem.session_id(), "custom-session-123");
}

#[test]
fn store_then_retrieve_string_roundtrip() {
    let (mem, _d) = temp_memory("agent-c");
    assert!(
        mem.store("greeting", "hello", MemoryType::Markdown)
            .unwrap()
    );
    let value = mem.retrieve("greeting").unwrap().expect("present");
    assert_eq!(value.as_str().unwrap(), "hello");
}

#[test]
fn store_then_retrieve_json_roundtrip() {
    let (mem, _d) = temp_memory("agent-d");
    let payload = serde_json::json!({"items": [1, 2, 3], "ok": true});
    assert!(
        mem.store_json("config", &payload, MemoryType::Json)
            .unwrap()
    );
    let got = mem.retrieve("config").unwrap().expect("present");
    assert_eq!(got, payload);
}

#[test]
fn store_rejects_empty_key() {
    let (mem, _d) = temp_memory("agent-e");
    let err = mem.store("", "v", MemoryType::Markdown).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("empty"));
}

#[test]
fn retrieve_missing_returns_none() {
    let (mem, _d) = temp_memory("agent-f");
    assert!(mem.retrieve("nope").unwrap().is_none());
}

#[test]
fn list_returns_only_current_session_keys() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("m.db");
    let s1 = AgentMemory::builder()
        .agent_name("a")
        .session_id("s1")
        .db_path(&db)
        .build()
        .unwrap();
    let s2 = AgentMemory::builder()
        .agent_name("a")
        .session_id("s2")
        .db_path(&db)
        .build()
        .unwrap();
    s1.store("k1", "v1", MemoryType::Markdown).unwrap();
    s2.store("k2", "v2", MemoryType::Markdown).unwrap();
    let keys: Vec<String> = s1.list().unwrap().into_iter().map(|e| e.key).collect();
    assert!(keys.contains(&"k1".to_string()));
    assert!(!keys.contains(&"k2".to_string()));
}

#[test]
fn delete_removes_entry() {
    let (mem, _d) = temp_memory("agent-g");
    mem.store("doomed", "x", MemoryType::Markdown).unwrap();
    assert!(mem.delete("doomed").unwrap());
    assert!(mem.retrieve("doomed").unwrap().is_none());
    // Second delete is a no-op false
    assert!(!mem.delete("doomed").unwrap());
}

#[test]
fn disabled_memory_is_no_op() {
    let dir = TempDir::new().unwrap();
    let mem = AgentMemory::builder()
        .agent_name("disabled")
        .db_path(dir.path().join("m.db"))
        .enabled(false)
        .build()
        .unwrap();
    assert!(!mem.is_enabled());
    assert!(!mem.store("k", "v", MemoryType::Markdown).unwrap());
    assert!(mem.retrieve("k").unwrap().is_none());
    assert!(mem.list().unwrap().is_empty());
}

#[test]
fn store_overwrites_existing_key() {
    let (mem, _d) = temp_memory("agent-h");
    mem.store("k", "first", MemoryType::Markdown).unwrap();
    mem.store("k", "second", MemoryType::Markdown).unwrap();
    let v = mem.retrieve("k").unwrap().unwrap();
    assert_eq!(v.as_str().unwrap(), "second");
}
