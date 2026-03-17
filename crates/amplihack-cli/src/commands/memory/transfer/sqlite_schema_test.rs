//! TDD tests for S-2a: `init_hierarchical_sqlite_schema` in
//! `transfer/sqlite_backend.rs`.
//!
//! The function and constants do NOT exist yet. These tests will fail to
//! compile (or fail at runtime) until the implementation is added.

use crate::commands::memory::transfer::sqlite_backend::{
    SQLITE_HIERARCHICAL_INDEXES, SQLITE_HIERARCHICAL_SCHEMA, init_hierarchical_sqlite_schema,
};

/// After calling `init_hierarchical_sqlite_schema`, all 6 expected tables must
/// exist in `sqlite_master`.
///
/// Expected tables (by design spec):
///   semantic_memories, episodic_memories, similar_to_edges, derives_from_edges,
///   supersedes_edges, transitioned_to_edges
#[test]
fn init_hierarchical_sqlite_schema_creates_all_tables() {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory SQLite");
    init_hierarchical_sqlite_schema(&conn)
        .expect("init_hierarchical_sqlite_schema must succeed on fresh DB");

    let expected_tables = [
        "semantic_memories",
        "episodic_memories",
        "similar_to_edges",
        "derives_from_edges",
        "supersedes_edges",
        "transitioned_to_edges",
    ];

    for table in &expected_tables {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .expect("sqlite_master query must succeed");
        assert_eq!(
            count, 1,
            "table '{table}' must exist after init_hierarchical_sqlite_schema"
        );
    }
}

/// Calling `init_hierarchical_sqlite_schema` twice on the same connection must
/// not return an error (schema uses `CREATE TABLE IF NOT EXISTS` or equivalent).
#[test]
fn init_hierarchical_sqlite_schema_is_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory SQLite");
    init_hierarchical_sqlite_schema(&conn).expect("first call must succeed");
    init_hierarchical_sqlite_schema(&conn).expect("second call must also succeed (idempotent)");
}

/// At least 14 indexes must be created after schema initialisation.
///
/// The design spec documents 14 indexes across the 6 tables.
#[test]
fn init_hierarchical_sqlite_schema_creates_indexes() {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory SQLite");
    init_hierarchical_sqlite_schema(&conn).expect("init_hierarchical_sqlite_schema must succeed");

    let index_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .expect("sqlite_master index query must succeed");

    assert!(
        index_count >= 14,
        "expected at least 14 indexes, found {index_count}"
    );
}

/// The `SQLITE_HIERARCHICAL_SCHEMA` constant must be non-empty, confirming
/// it is defined and exported.
#[test]
fn sqlite_hierarchical_schema_constant_is_non_empty() {
    assert!(
        !SQLITE_HIERARCHICAL_SCHEMA.is_empty(),
        "SQLITE_HIERARCHICAL_SCHEMA must not be empty"
    );
}

/// The `SQLITE_HIERARCHICAL_INDEXES` constant must contain at least 14 entries.
#[test]
fn sqlite_hierarchical_indexes_constant_has_at_least_14_entries() {
    // Count the number of CREATE INDEX statements.
    let count = SQLITE_HIERARCHICAL_INDEXES
        .iter()
        .filter(|s| {
            let upper = s.to_uppercase();
            upper.contains("CREATE INDEX") || upper.contains("CREATE UNIQUE INDEX")
        })
        .count();
    assert!(
        count >= 14,
        "SQLITE_HIERARCHICAL_INDEXES must define at least 14 indexes, found {count}"
    );
}
