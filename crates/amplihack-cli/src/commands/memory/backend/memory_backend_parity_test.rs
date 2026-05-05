//! Backend parity tests: every trait method exercised against both
//! `SqliteBackend` and `GraphDbBackend` to confirm identical semantics.
//!
//! Each test is parameterised over a pair of temporary databases – one per
//! backend – and asserts that the observable behaviour is the same for both.
//!
//! ## Test coverage
//! - `MemoryTreeBackend::load_session_rows` – unfiltered, session-id filtered,
//!   `memory_type` filtered
//! - `MemoryTreeBackend::collect_agent_counts`
//! - `MemorySessionBackend::list_sessions` / `delete_session`
//! - `MemoryRuntimeBackend::store_session_learning` / `load_prompt_context_memories`
//! - Expiration: expired records are **never** returned by either backend

use super::super::*;
use super::{MemoryRuntimeBackend, open_cleanup_backend, open_runtime_backend, open_tree_backend};
use crate::test_support::home_env_lock;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Guard that sets `HOME` and `AMPLIHACK_GRAPH_DB_PATH` for the duration of a
/// test, restoring them when dropped.
struct EnvGuard {
    prev_home: Option<std::ffi::OsString>,
    prev_graph: Option<std::ffi::OsString>,
    prev_kuzu: Option<std::ffi::OsString>,
    prev_backend: Option<std::ffi::OsString>,
    #[allow(dead_code)]
    lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn setup(home: &std::path::Path, graph_db: &std::path::Path, backend: &str) -> Self {
        let lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var_os("HOME");
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", graph_db);
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
            std::env::set_var("AMPLIHACK_MEMORY_BACKEND", backend);
        }
        Self {
            prev_home,
            prev_graph,
            prev_kuzu,
            prev_backend,
            lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.prev_home.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match self.prev_graph.take() {
                Some(v) => std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v),
                None => std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH"),
            }
            match self.prev_kuzu.take() {
                Some(v) => std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v),
                None => std::env::remove_var("AMPLIHACK_KUZU_DB_PATH"),
            }
            match self.prev_backend.take() {
                Some(v) => std::env::set_var("AMPLIHACK_MEMORY_BACKEND", v),
                None => std::env::remove_var("AMPLIHACK_MEMORY_BACKEND"),
            }
        }
    }
}

/// Insert a minimal learning record via the runtime backend and return its id.
fn seed_learning(
    backend: &dyn MemoryRuntimeBackend,
    session_id: &str,
    agent_id: &str,
    content: &str,
    memory_type_tag: &str,
) -> anyhow::Result<String> {
    let record = SessionLearningRecord {
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        content: content.to_string(),
        title: content.chars().take(30).collect(),
        metadata: serde_json::json!({
            "new_memory_type": memory_type_tag,
            "tags": ["parity_test"]
        }),
        importance: 6,
    };
    backend
        .store_session_learning(&record)?
        .ok_or_else(|| anyhow::anyhow!("store_session_learning returned None (duplicate?)"))
}

// ---------------------------------------------------------------------------
// SQLite helpers
// ---------------------------------------------------------------------------

/// Seed the SQLite backend directly via raw SQL for richer test scenarios
/// (e.g. setting explicit `expires_at`).
fn sqlite_insert_memory(
    conn: &rusqlite::Connection,
    id: &str,
    session_id: &str,
    agent_id: &str,
    memory_type: &str,
    content: &str,
    expires_at: Option<&str>,
) -> anyhow::Result<()> {
    use rusqlite::params;
    let now = "2026-01-01T12:00:00";
    conn.execute(
        "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}') ON CONFLICT DO NOTHING",
        params![session_id, now, now],
    )?;
    conn.execute(
        "INSERT INTO memory_entries \
         (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{}', 6, ?7, ?8, ?9)",
        params![id, session_id, agent_id, memory_type, content, content, now, now, expires_at],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// store_session_learning / load_prompt_context_memories parity
// ---------------------------------------------------------------------------

/// Both backends should store a learning record and retrieve it in the same
/// call to `load_prompt_context_memories`.
///
/// We open a **fresh** backend for the read so that both backends see
/// committed data regardless of whether the write path uses a different
/// internal database handle (as Kùzu's `store_learning_graph` does).
fn assert_runtime_round_trip(choice: BackendChoice) -> anyhow::Result<()> {
    let content =
        "Agent analyzer: parity test content — confirm round trip for backend round-trip test.";
    {
        let backend = open_runtime_backend(choice)?;
        let _id = seed_learning(&*backend, "sess-rt", "analyzer", content, "semantic")?;
    } // backend (and its cached DB handle) dropped here

    // Open a fresh backend so it sees the committed write.
    let backend2 = open_runtime_backend(choice)?;
    let memories = backend2.load_prompt_context_memories("sess-rt")?;
    assert!(
        !memories.is_empty(),
        "[{choice:?}] expected at least one memory after store"
    );
    let found = memories
        .iter()
        .any(|m| m.content.contains("parity test content"));
    assert!(
        found,
        "[{choice:?}] stored content not found in load_prompt_context_memories"
    );
    Ok(())
}

#[test]
fn sqlite_runtime_round_trip() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "sqlite");
    assert_runtime_round_trip(BackendChoice::Sqlite)
}

#[test]
fn graph_db_runtime_round_trip() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "graph-db");
    assert_runtime_round_trip(BackendChoice::GraphDb)
}

// ---------------------------------------------------------------------------
// duplicate detection parity
// ---------------------------------------------------------------------------

/// Both backends should return `None` on a duplicate store attempt.
fn assert_duplicate_returns_none(choice: BackendChoice) -> anyhow::Result<()> {
    let backend = open_runtime_backend(choice)?;
    let content = "Agent dup-agent: identical content stored twice to confirm dedup.";
    let id1 = seed_learning(&*backend, "sess-dup", "dup-agent", content, "semantic")?;
    let record2 = SessionLearningRecord {
        session_id: "sess-dup".to_string(),
        agent_id: "dup-agent".to_string(),
        content: content.to_string(),
        title: "dup title".to_string(),
        metadata: serde_json::json!({ "new_memory_type": "semantic" }),
        importance: 6,
    };
    let second = backend.store_session_learning(&record2)?;
    assert!(
        second.is_none(),
        "[{choice:?}] expected None for duplicate store, got {second:?}"
    );
    assert!(!id1.is_empty());
    Ok(())
}

#[test]
fn sqlite_duplicate_returns_none() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "sqlite");
    assert_duplicate_returns_none(BackendChoice::Sqlite)
}

#[test]
fn graph_db_duplicate_returns_none() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "graph-db");
    assert_duplicate_returns_none(BackendChoice::GraphDb)
}

// ---------------------------------------------------------------------------
// load_session_rows – session_id filter parity
// ---------------------------------------------------------------------------

/// `load_session_rows(Some(session_id), None)` must return only that session.
fn assert_session_id_filter(choice: BackendChoice) -> anyhow::Result<()> {
    let backend_rt = open_runtime_backend(choice)?;
    seed_learning(
        &*backend_rt,
        "sess-A",
        "ag1",
        "Agent ag1: first session content parity.",
        "semantic",
    )?;
    seed_learning(
        &*backend_rt,
        "sess-B",
        "ag1",
        "Agent ag1: second session content parity.",
        "semantic",
    )?;

    let tree = open_tree_backend(choice)?;
    let rows_a = tree.load_session_rows(Some("sess-A"), None)?;
    assert_eq!(
        rows_a.len(),
        1,
        "[{:?}] expected 1 session row for sess-A, got {}",
        choice,
        rows_a.len()
    );
    assert_eq!(
        rows_a[0].0.session_id, "sess-A",
        "[{choice:?}] session_id mismatch"
    );

    let rows_all = tree.load_session_rows(None, None)?;
    assert!(
        rows_all.len() >= 2,
        "[{:?}] expected >= 2 sessions without filter, got {}",
        choice,
        rows_all.len()
    );
    Ok(())
}

#[test]
fn sqlite_session_id_filter() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "sqlite");
    assert_session_id_filter(BackendChoice::Sqlite)
}

#[test]
fn graph_db_session_id_filter() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "graph-db");
    assert_session_id_filter(BackendChoice::GraphDb)
}

// ---------------------------------------------------------------------------
// load_session_rows – memory_type filter parity
// ---------------------------------------------------------------------------

/// `load_session_rows(None, Some("episodic"))` must return only episodic
/// records (AC: "returns only episodic records, not all records").
///
/// For the SQLite backend we insert directly so we can control the
/// `memory_type` column.  For the Kuzu backend the schema models memory
/// types as distinct node tables; the parity test seeds an EpisodicMemory
/// node via a direct Kuzu statement and checks the filter works.
#[test]
fn sqlite_memory_type_filter_episodic() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "sqlite");

    // Open SQLite and seed two different memory types.
    let conn = open_sqlite_memory_db()?;
    sqlite_insert_memory(
        &conn,
        "m-ep1",
        "sess-filter",
        "ag1",
        "episodic",
        "episodic content",
        None,
    )?;
    sqlite_insert_memory(
        &conn,
        "m-sem1",
        "sess-filter",
        "ag1",
        "semantic",
        "semantic content",
        None,
    )?;

    let tree = open_tree_backend(BackendChoice::Sqlite)?;

    let episodic_rows = tree.load_session_rows(None, Some("episodic"))?;
    for (_, memories) in &episodic_rows {
        for m in memories {
            assert_eq!(
                m.memory_type, "episodic",
                "SQLite filter returned non-episodic record: {:?}",
                m.memory_type
            );
        }
    }
    let has_episodic = episodic_rows
        .iter()
        .any(|(_, mems)| mems.iter().any(|m| m.memory_id == "m-ep1"));
    assert!(
        has_episodic,
        "SQLite episodic filter did not return seeded episodic record"
    );

    let semantic_rows = tree.load_session_rows(None, Some("semantic"))?;
    let has_semantic = semantic_rows
        .iter()
        .any(|(_, mems)| mems.iter().any(|m| m.memory_id == "m-sem1"));
    assert!(
        has_semantic,
        "SQLite semantic filter did not return seeded semantic record"
    );
    Ok(())
}

#[test]
fn graph_db_memory_type_filter_episodic() -> anyhow::Result<()> {
    use crate::commands::memory::backend::graph_db::{
        GraphDbHandle, GraphDbValue, graph_rows, init_graph_backend_schema,
    };

    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "graph-db");

    // Seed EpisodicMemory and SemanticMemory directly.
    GraphDbHandle::open_at_path(&graph)?.with_conn(|conn| {
        init_graph_backend_schema(conn)?;

        // Seed a Session node.
        graph_rows(
            conn,
            "CREATE (s:Session {session_id: $sid, start_time: $t, end_time: NULL, user_id: '', context: '', status: 'active', created_at: $t, last_accessed: $t, metadata: '{}'})",
            vec![
                ("sid", GraphDbValue::String("sess-kf".to_string())),
                (
                    "t",
                    GraphDbValue::Timestamp(time::OffsetDateTime::now_utc()),
                ),
            ],
        )?;

        // Seed an EpisodicMemory node.
        graph_rows(
            conn,
            "CREATE (m:EpisodicMemory {memory_id: 'm-ep-graph-db', timestamp: $t, content: 'episodic parity content', event_type: 'test', emotional_valence: 0.0, importance_score: 6.0, title: 'ep title', metadata: '{}', tags: '[]', created_at: $t, accessed_at: $t, expires_at: NULL, agent_id: 'ag1'})",
            vec![(
                "t",
                GraphDbValue::Timestamp(time::OffsetDateTime::now_utc()),
            )],
        )?;
        graph_rows(
            conn,
            "MATCH (s:Session {session_id: 'sess-kf'}), (m:EpisodicMemory {memory_id: 'm-ep-graph-db'}) CREATE (s)-[:CONTAINS_EPISODIC {sequence_number: 1}]->(m)",
            vec![],
        )?;

        // Seed a SemanticMemory node.
        let now_str = chrono::Utc::now().to_rfc3339();
        let now_ts = time::OffsetDateTime::now_utc();
        graph_rows(
            conn,
            "CREATE (m:SemanticMemory {memory_id: 'm-sem-graph-db', concept: 'test concept', content: 'semantic parity content', category: 'test', confidence_score: 1.0, last_updated: $t, version: 1, title: 'sem title', metadata: '{}', tags: '[]', created_at: $t, accessed_at: $t, agent_id: 'ag1'})",
            vec![("t", GraphDbValue::Timestamp(now_ts))],
        )?;
        graph_rows(
            conn,
            "MATCH (s:Session {session_id: 'sess-kf'}), (m:SemanticMemory {memory_id: 'm-sem-graph-db'}) CREATE (s)-[:CONTRIBUTES_TO_SEMANTIC {contribution_type: 'created', timestamp: $t, delta: 'initial'}]->(m)",
            vec![("t", GraphDbValue::Timestamp(now_ts))],
        )?;
        drop(now_str);
        Ok(())
    })?;

    // Now use the tree backend to filter.
    let tree = open_tree_backend(BackendChoice::GraphDb)?;

    let episodic_rows = tree.load_session_rows(None, Some("episodic"))?;
    for (_, memories) in &episodic_rows {
        for m in memories {
            assert_eq!(
                m.memory_type, "episodic",
                "Graph DB filter returned non-episodic record: {:?}",
                m.memory_type
            );
        }
    }
    let has_ep = episodic_rows
        .iter()
        .any(|(_, mems)| mems.iter().any(|m| m.memory_id == "m-ep-graph-db"));
    assert!(
        has_ep,
        "Graph DB episodic filter did not return seeded episodic memory"
    );

    let semantic_rows = tree.load_session_rows(None, Some("semantic"))?;
    let has_sem = semantic_rows
        .iter()
        .any(|(_, mems)| mems.iter().any(|m| m.memory_id == "m-sem-graph-db"));
    assert!(
        has_sem,
        "Graph DB semantic filter did not return seeded semantic memory"
    );

    // Cross-check: no episodic memory should appear in semantic results.
    for (_, memories) in &semantic_rows {
        for m in memories {
            assert_ne!(
                m.memory_id, "m-ep-graph-db",
                "Graph DB semantic filter leaked episodic record"
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// collect_agent_counts parity
// ---------------------------------------------------------------------------

fn assert_collect_agent_counts(choice: BackendChoice) -> anyhow::Result<()> {
    let backend_rt = open_runtime_backend(choice)?;
    seed_learning(
        &*backend_rt,
        "sess-cnt",
        "cnt-agent",
        "Agent cnt-agent: first learning for count test.",
        "semantic",
    )?;
    seed_learning(
        &*backend_rt,
        "sess-cnt",
        "cnt-agent",
        "Agent cnt-agent: second distinct learning for count test.",
        "semantic",
    )?;

    let tree = open_tree_backend(choice)?;
    let counts = tree.collect_agent_counts()?;
    let cnt_entry = counts.iter().find(|(id, _)| id == "cnt-agent");
    assert!(
        cnt_entry.is_some(),
        "[{choice:?}] cnt-agent not found in agent counts"
    );
    let (_, count) = cnt_entry.unwrap();
    assert!(
        *count >= 1,
        "[{choice:?}] expected count >= 1 for cnt-agent, got {count}"
    );
    Ok(())
}

#[test]
fn sqlite_collect_agent_counts() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "sqlite");
    assert_collect_agent_counts(BackendChoice::Sqlite)
}

#[test]
fn graph_db_collect_agent_counts() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "graph-db");
    assert_collect_agent_counts(BackendChoice::GraphDb)
}

// ---------------------------------------------------------------------------
// list_sessions / delete_session parity
// ---------------------------------------------------------------------------

/// Open a fresh cleanup backend and list sessions.
fn fresh_list(choice: BackendChoice) -> anyhow::Result<Vec<SessionSummary>> {
    open_cleanup_backend(choice)?.list_sessions()
}

/// Open a fresh cleanup backend and delete a session.
fn fresh_delete(choice: BackendChoice, session_id: &str) -> anyhow::Result<bool> {
    open_cleanup_backend(choice)?.delete_session(session_id)
}

fn assert_list_and_delete_session(choice: BackendChoice) -> anyhow::Result<()> {
    // Write then drop so the cached DB handle is released before any reads.
    {
        let backend_rt = open_runtime_backend(choice)?;
        seed_learning(
            &*backend_rt,
            "sess-del",
            "del-agent",
            "Agent del-agent: session to be deleted.",
            "semantic",
        )?;
    }

    // Each operation uses a fresh backend so it sees the latest committed state.
    let sessions_before = fresh_list(choice)?;
    let had_session = sessions_before.iter().any(|s| s.session_id == "sess-del");
    assert!(
        had_session,
        "[{choice:?}] expected sess-del in list before delete"
    );

    let deleted = fresh_delete(choice, "sess-del")?;
    assert!(
        deleted,
        "[{choice:?}] delete_session should return true for existing session"
    );

    let sessions_after = fresh_list(choice)?;
    let still_present = sessions_after.iter().any(|s| s.session_id == "sess-del");
    assert!(
        !still_present,
        "[{choice:?}] sess-del should not appear after deletion"
    );

    // Deleting again should return false.
    let deleted_again = fresh_delete(choice, "sess-del")?;
    assert!(
        !deleted_again,
        "[{choice:?}] second delete should return false"
    );
    Ok(())
}

#[test]
fn sqlite_list_and_delete_session() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "sqlite");
    assert_list_and_delete_session(BackendChoice::Sqlite)
}

#[test]
fn graph_db_list_and_delete_session() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "graph-db");
    assert_list_and_delete_session(BackendChoice::GraphDb)
}

// ---------------------------------------------------------------------------
// Expiration parity: expired records must NOT be returned
// ---------------------------------------------------------------------------

/// SQLite: insert a record with `expires_at` in the past and verify it is
/// excluded from `load_session_rows` and `collect_agent_counts`.
#[test]
fn sqlite_expired_records_not_returned() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "sqlite");

    let conn = open_sqlite_memory_db()?;
    // Expired record (expires_at in the past).
    sqlite_insert_memory(
        &conn,
        "m-expired",
        "sess-exp",
        "exp-agent",
        "episodic",
        "expired content",
        Some("2000-01-01T00:00:00"),
    )?;
    // Non-expired record.
    sqlite_insert_memory(
        &conn,
        "m-valid",
        "sess-exp",
        "exp-agent",
        "episodic",
        "valid content",
        Some("2099-12-31T23:59:59"),
    )?;

    let tree = open_tree_backend(BackendChoice::Sqlite)?;
    let rows = tree.load_session_rows(Some("sess-exp"), None)?;
    let all_memories: Vec<_> = rows.iter().flat_map(|(_, mems)| mems.iter()).collect();

    let expired_present = all_memories.iter().any(|m| m.memory_id == "m-expired");
    let valid_present = all_memories.iter().any(|m| m.memory_id == "m-valid");

    assert!(!expired_present, "SQLite returned expired record m-expired");
    assert!(valid_present, "SQLite did not return valid record m-valid");

    // Also check agent counts exclude expired.
    let counts = tree.collect_agent_counts()?;
    let exp_count = counts.iter().find(|(id, _)| id == "exp-agent");
    if let Some((_, count)) = exp_count {
        assert_eq!(
            *count, 1,
            "SQLite agent count should be 1 (only valid), got {count}"
        );
    }
    Ok(())
}

/// Kuzu: insert EpisodicMemory and WorkingMemory records with `expires_at` in
/// the past; verify they are excluded from `load_session_rows` and
/// `collect_agent_counts`.
#[test]
fn graph_db_expired_records_not_returned() -> anyhow::Result<()> {
    use crate::commands::memory::backend::graph_db::{
        GraphDbHandle, GraphDbValue, graph_rows, init_graph_backend_schema,
    };

    let dir = tempfile::tempdir()?;
    let graph = dir.path().join("graph.db");
    let _guard = EnvGuard::setup(dir.path(), &graph, "graph-db");

    let now_ts = time::OffsetDateTime::now_utc();
    GraphDbHandle::open_at_path(&graph)?.with_conn(|conn| {
        init_graph_backend_schema(conn)?;

        // Seed the session.
        graph_rows(
            conn,
            "CREATE (s:Session {session_id: 'sess-exp-k', start_time: $t, end_time: NULL, user_id: '', context: '', status: 'active', created_at: $t, last_accessed: $t, metadata: '{}'})",
            vec![("t", GraphDbValue::Timestamp(now_ts))],
        )?;

        // Past timestamp for expires_at.
        let past: time::OffsetDateTime =
            time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(365); // 1971-01-01

        // Expired EpisodicMemory.
        graph_rows(
            conn,
            "CREATE (m:EpisodicMemory {memory_id: 'ep-expired', timestamp: $t, content: 'expired ep', event_type: 'test', emotional_valence: 0.0, importance_score: 5.0, title: 'expired', metadata: '{}', tags: '[]', created_at: $t, accessed_at: $t, expires_at: $exp, agent_id: 'exp-agent'})",
            vec![
                ("t", GraphDbValue::Timestamp(now_ts)),
                ("exp", GraphDbValue::Timestamp(past)),
            ],
        )?;
        graph_rows(
            conn,
            "MATCH (s:Session {session_id: 'sess-exp-k'}), (m:EpisodicMemory {memory_id: 'ep-expired'}) CREATE (s)-[:CONTAINS_EPISODIC {sequence_number: 1}]->(m)",
            vec![],
        )?;

        // Valid EpisodicMemory (no expires_at = never expires).
        graph_rows(
            conn,
            "CREATE (m:EpisodicMemory {memory_id: 'ep-valid', timestamp: $t, content: 'valid ep', event_type: 'test', emotional_valence: 0.0, importance_score: 6.0, title: 'valid', metadata: '{}', tags: '[]', created_at: $t, accessed_at: $t, expires_at: NULL, agent_id: 'exp-agent'})",
            vec![("t", GraphDbValue::Timestamp(now_ts))],
        )?;
        graph_rows(
            conn,
            "MATCH (s:Session {session_id: 'sess-exp-k'}), (m:EpisodicMemory {memory_id: 'ep-valid'}) CREATE (s)-[:CONTAINS_EPISODIC {sequence_number: 2}]->(m)",
            vec![],
        )?;
        Ok(())
    })?;

    let tree = open_tree_backend(BackendChoice::GraphDb)?;
    let rows = tree.load_session_rows(Some("sess-exp-k"), None)?;
    let all_memories: Vec<_> = rows.iter().flat_map(|(_, mems)| mems.iter()).collect();

    let expired_present = all_memories.iter().any(|m| m.memory_id == "ep-expired");
    let valid_present = all_memories.iter().any(|m| m.memory_id == "ep-valid");

    assert!(
        !expired_present,
        "graph-db backend returned expired record ep-expired; memories: {:?}",
        all_memories
            .iter()
            .map(|m| &m.memory_id)
            .collect::<Vec<_>>()
    );
    assert!(
        valid_present,
        "graph-db backend did not return valid record ep-valid"
    );

    // collect_agent_counts should exclude expired.
    let counts = tree.collect_agent_counts()?;
    let exp_agent_count = counts.iter().find(|(id, _)| id == "exp-agent");
    if let Some((_, count)) = exp_agent_count {
        assert_eq!(
            *count, 1,
            "graph-db agent count should be 1 (only valid ep), got {count}"
        );
    }
    Ok(())
}
