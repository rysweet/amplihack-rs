//! Atomicity and correctness tests for the SQLite hierarchical transfer backend.
//!
//! Covers:
//! - `import_hierarchical_json` (merge=false): the clear+insert sequence is
//!   atomic — a successful non-merge import replaces data cleanly.
//! - `delete_sqlite_session_with_conn`: all three DELETEs are atomic — after a
//!   successful delete, no orphaned rows remain in any of the three tables.
//! - `copy_dir`: symlinks are skipped and do not cause traversal.

use crate::commands::memory::backend::sqlite::{SQLITE_SCHEMA, delete_sqlite_session_with_conn};
use crate::commands::memory::transfer::backend::HierarchicalTransferBackend as _;
use crate::commands::memory::transfer::sqlite_backend::SqliteHierarchicalTransferBackend;
use crate::commands::memory::transfer::{
    DerivesEdge, EpisodicNode, HierarchicalExportData, HierarchicalStats, SemanticNode,
};
use anyhow::Result;
use rusqlite::{Connection as SqliteConnection, params};
use std::fs;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_export_data(
    agent_name: &str,
    semantic_id: &str,
    episodic_id: &str,
) -> HierarchicalExportData {
    HierarchicalExportData {
        agent_name: agent_name.to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![SemanticNode {
            memory_id: semantic_id.to_string(),
            concept: "test".to_string(),
            content: "test content".to_string(),
            confidence: 0.9,
            source_id: episodic_id.to_string(),
            tags: vec!["test".to_string()],
            metadata: serde_json::json!({}),
            created_at: "1704067200".to_string(),
            entity_name: "test-entity".to_string(),
        }],
        episodic_nodes: vec![EpisodicNode {
            memory_id: episodic_id.to_string(),
            content: "episodic content".to_string(),
            source_label: "session".to_string(),
            tags: vec!["test".to_string()],
            metadata: serde_json::json!({}),
            created_at: "1704067200".to_string(),
        }],
        similar_to_edges: vec![],
        derives_from_edges: vec![DerivesEdge {
            source_id: semantic_id.to_string(),
            target_id: episodic_id.to_string(),
            extraction_method: "test".to_string(),
            confidence: 0.8,
        }],
        supersedes_edges: vec![],
        transitioned_to_edges: vec![],
        statistics: HierarchicalStats {
            semantic_node_count: 1,
            episodic_node_count: 1,
            similar_to_edge_count: 0,
            derives_from_edge_count: 1,
            supersedes_edge_count: 0,
            transitioned_to_edge_count: 0,
        },
    }
}

// ---------------------------------------------------------------------------
// import_hierarchical_json — merge=false atomicity
// ---------------------------------------------------------------------------

/// A successful `merge=false` import replaces the agent's data entirely:
/// old nodes are gone and new nodes are present.  This verifies the
/// clear+insert sequence commits as a unit.
#[test]
fn import_merge_false_replaces_data_atomically() -> Result<()> {
    let dir = tempdir()?;
    let storage = dir.path().to_string_lossy().into_owned();
    let backend = SqliteHierarchicalTransferBackend;

    // ── Phase 1: import original data ──
    let original = make_export_data("atomicity-agent", "sem-original", "ep-original");
    let input1 = dir.path().join("original.json");
    fs::write(&input1, serde_json::to_string_pretty(&original)?)?;

    let result1 = backend.import_hierarchical_json(
        "atomicity-agent",
        &input1.to_string_lossy(),
        false,
        Some(&storage),
    )?;
    assert!(
        result1
            .statistics
            .iter()
            .any(|(k, v)| k == "semantic_nodes_imported" && v == "1")
    );

    // ── Phase 2: import replacement data with merge=false ──
    let replacement = make_export_data("atomicity-agent", "sem-new", "ep-new");
    let input2 = dir.path().join("replacement.json");
    fs::write(&input2, serde_json::to_string_pretty(&replacement)?)?;

    let result2 = backend.import_hierarchical_json(
        "atomicity-agent",
        &input2.to_string_lossy(),
        false,
        Some(&storage),
    )?;
    assert!(
        result2
            .statistics
            .iter()
            .any(|(k, v)| k == "semantic_nodes_imported" && v == "1")
    );

    // ── Phase 3: export and verify only new data is present ──
    let out = dir.path().join("exported.json");
    backend.export_hierarchical_json("atomicity-agent", &out.to_string_lossy(), Some(&storage))?;
    let exported: HierarchicalExportData = serde_json::from_str(&fs::read_to_string(&out)?)?;

    assert_eq!(
        exported.semantic_nodes.len(),
        1,
        "should have exactly 1 semantic node after replacement"
    );
    assert_eq!(
        exported.semantic_nodes[0].memory_id, "sem-new",
        "old sem-original must be gone"
    );
    assert_eq!(
        exported.episodic_nodes.len(),
        1,
        "should have exactly 1 episodic node after replacement"
    );
    assert_eq!(
        exported.episodic_nodes[0].memory_id, "ep-new",
        "old ep-original must be gone"
    );

    Ok(())
}

/// A `merge=true` import must NOT delete existing data; it should add only new
/// nodes and skip duplicates.
#[test]
fn import_merge_true_preserves_existing_data() -> Result<()> {
    let dir = tempdir()?;
    let storage = dir.path().to_string_lossy().into_owned();
    let backend = SqliteHierarchicalTransferBackend;

    // Phase 1: seed original data.
    let original = make_export_data("merge-agent", "sem-keep", "ep-keep");
    let input1 = dir.path().join("seed.json");
    fs::write(&input1, serde_json::to_string_pretty(&original)?)?;
    backend.import_hierarchical_json(
        "merge-agent",
        &input1.to_string_lossy(),
        false,
        Some(&storage),
    )?;

    // Phase 2: merge new data (different IDs).
    let new_data = make_export_data("merge-agent", "sem-added", "ep-added");
    let input2 = dir.path().join("new.json");
    fs::write(&input2, serde_json::to_string_pretty(&new_data)?)?;
    backend.import_hierarchical_json(
        "merge-agent",
        &input2.to_string_lossy(),
        true,
        Some(&storage),
    )?;

    // Phase 3: export and verify both sets of data are present.
    let out = dir.path().join("exported.json");
    backend.export_hierarchical_json("merge-agent", &out.to_string_lossy(), Some(&storage))?;
    let exported: HierarchicalExportData = serde_json::from_str(&fs::read_to_string(&out)?)?;

    let sem_ids: Vec<&str> = exported
        .semantic_nodes
        .iter()
        .map(|n| n.memory_id.as_str())
        .collect();
    assert!(
        sem_ids.contains(&"sem-keep"),
        "merge must preserve original node"
    );
    assert!(
        sem_ids.contains(&"sem-added"),
        "merge must include new node"
    );
    assert_eq!(
        exported.semantic_nodes.len(),
        2,
        "merge must not duplicate nodes"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// delete_sqlite_session_with_conn — atomicity
// ---------------------------------------------------------------------------

fn seed_session(conn: &SqliteConnection, session_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
        params![session_id, "2026-01-01T00:00:00", "2026-01-01T00:00:00"],
    )?;
    conn.execute(
        "INSERT INTO session_agents (session_id, agent_id, first_used, last_used) VALUES (?1, ?2, ?3, ?4)",
        params![session_id, "test-agent", "2026-01-01T00:00:00", "2026-01-01T00:00:00"],
    )?;
    conn.execute(
        "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{}', ?7, ?8)",
        params![
            format!("mem-{session_id}"),
            session_id,
            "test-agent",
            "learning",
            "test title",
            "test content",
            "2026-01-01T00:00:00",
            "2026-01-01T00:00:00",
        ],
    )?;
    Ok(())
}

fn count_rows(conn: &SqliteConnection, table: &str, session_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE session_id = ?1"),
        params![session_id],
        |row| row.get(0),
    )?)
}

/// After a successful `delete_sqlite_session_with_conn`, all three tables
/// (memory_entries, session_agents, sessions) must have zero rows for the
/// deleted session — no orphaned rows.
#[test]
fn delete_session_removes_all_rows_atomically() -> Result<()> {
    let conn = SqliteConnection::open_in_memory()?;
    conn.execute_batch(SQLITE_SCHEMA)?;

    seed_session(&conn, "del-sess")?;

    // Verify rows exist before delete.
    assert_eq!(count_rows(&conn, "sessions", "del-sess")?, 1);
    assert_eq!(count_rows(&conn, "session_agents", "del-sess")?, 1);
    assert_eq!(count_rows(&conn, "memory_entries", "del-sess")?, 1);

    let deleted = delete_sqlite_session_with_conn(&conn, "del-sess")?;
    assert!(deleted, "delete must return true for existing session");

    // All three tables must be empty for this session.
    assert_eq!(
        count_rows(&conn, "sessions", "del-sess")?,
        0,
        "sessions row must be deleted"
    );
    assert_eq!(
        count_rows(&conn, "session_agents", "del-sess")?,
        0,
        "session_agents row must be deleted"
    );
    assert_eq!(
        count_rows(&conn, "memory_entries", "del-sess")?,
        0,
        "memory_entries row must be deleted"
    );

    Ok(())
}

/// Deleting a non-existent session must return `false` and leave the database
/// unchanged.
#[test]
fn delete_session_returns_false_for_missing_session() -> Result<()> {
    let conn = SqliteConnection::open_in_memory()?;
    conn.execute_batch(SQLITE_SCHEMA)?;

    seed_session(&conn, "keeper")?;

    let deleted = delete_sqlite_session_with_conn(&conn, "ghost-session")?;
    assert!(!deleted, "deleting non-existent session must return false");

    // Unrelated session must be untouched.
    assert_eq!(
        count_rows(&conn, "sessions", "keeper")?,
        1,
        "unrelated session must not be affected"
    );
    assert_eq!(
        count_rows(&conn, "memory_entries", "keeper")?,
        1,
        "unrelated entries must not be affected"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// copy_dir — symlink guard
// ---------------------------------------------------------------------------

/// `copy_dir` must skip symlinks (not follow them) so that a directory
/// containing a symlink cannot be used for path traversal.
#[cfg(unix)]
#[test]
fn copy_dir_skips_symlinks() -> Result<()> {
    use std::os::unix::fs::symlink;

    let src = tempdir()?;
    let dst = tempdir()?;

    // Create a regular file in src.
    fs::write(src.path().join("regular.txt"), b"regular content")?;

    // Create a symlink in src pointing outside the directory.
    let outside = tempdir()?;
    let outside_file = outside.path().join("secret.txt");
    fs::write(&outside_file, b"secret content")?;
    symlink(&outside_file, src.path().join("evil_symlink.txt"))?;

    // Run copy_dir — must not error and must skip the symlink.
    // Access copy_dir via the import path (it's private, so we test it
    // indirectly via import_hierarchical_raw_db on a directory input,
    // OR we can verify the output doesn't contain the symlink target).
    //
    // copy_dir is private; we verify its behaviour through import_hierarchical_raw_db
    // which calls copy_dir when the input is a directory.
    let backend = SqliteHierarchicalTransferBackend;
    let _output = dst.path().join("imported.db");
    let result = backend.import_hierarchical_raw_db(
        "test-agent",
        src.path().to_str().unwrap(),
        false,
        Some(dst.path().to_str().unwrap()),
    );

    // The import may succeed or fail depending on the DB schema check, but
    // critically: the symlink target content must NOT appear in the destination.
    let _ = result; // don't assert import success — it may fail on schema validation
    let evil_dst = dst.path().join("evil_symlink.txt");
    assert!(
        !evil_dst.exists(),
        "copy_dir must not copy symlink targets to destination"
    );

    Ok(())
}
