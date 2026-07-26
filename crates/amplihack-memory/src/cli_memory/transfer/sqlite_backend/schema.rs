//! Schema constants and initialisation for the hierarchical SQLite backend.

use anyhow::{Context, Result};
use rusqlite::Connection as SqliteConnection;

/// Maximum allowed JSON file size: 500 MiB.
pub(crate) const MAX_JSON_FILE_SIZE: u64 = 500 * 1024 * 1024;

/// Maximum agent name length (filesystem constraint).
pub(crate) const MAX_AGENT_NAME_LEN: usize = 255;

/// CREATE TABLE statements for the hierarchical SQLite schema.
///
/// Six tables matching the graph-db node/rel tables:
///   semantic_memories, episodic_memories, similar_to_edges, derives_from_edges,
///   supersedes_edges, transitioned_to_edges
pub(crate) const SQLITE_HIERARCHICAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS semantic_memories (
    memory_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    concept TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL DEFAULT '',
    confidence REAL NOT NULL DEFAULT 0.0,
    source_id TEXT NOT NULL DEFAULT '',
    tags TEXT NOT NULL DEFAULT '[]',
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT '',
    entity_name TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS episodic_memories (
    memory_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    source_label TEXT NOT NULL DEFAULT '',
    tags TEXT NOT NULL DEFAULT '[]',
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS similar_to_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    weight REAL NOT NULL DEFAULT 0.0,
    metadata TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS derives_from_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    extraction_method TEXT NOT NULL DEFAULT '',
    confidence REAL NOT NULL DEFAULT 0.0
);
CREATE TABLE IF NOT EXISTS supersedes_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    temporal_delta TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS transitioned_to_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    from_value TEXT NOT NULL DEFAULT '',
    to_value TEXT NOT NULL DEFAULT '',
    turn INTEGER NOT NULL DEFAULT 0,
    transition_type TEXT NOT NULL DEFAULT ''
);
"#;

/// A collection of SQL index statements that provides an iterator yielding
/// `&str` items (so that `.filter(|s: &&str| ...)` works correctly in tests).
pub(crate) struct SqlIndexStatements(pub &'static [&'static str]);

impl SqlIndexStatements {
    /// Returns an iterator over the index statements, yielding `&str`.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().copied()
    }
}

/// CREATE INDEX statements (14 indexes across the 6 tables).
pub(crate) const SQLITE_HIERARCHICAL_INDEXES: SqlIndexStatements = SqlIndexStatements(&[
    "CREATE INDEX IF NOT EXISTS idx_semantic_agent ON semantic_memories(agent_id)",
    "CREATE INDEX IF NOT EXISTS idx_semantic_concept ON semantic_memories(concept)",
    "CREATE INDEX IF NOT EXISTS idx_semantic_created ON semantic_memories(created_at)",
    "CREATE INDEX IF NOT EXISTS idx_episodic_agent ON episodic_memories(agent_id)",
    "CREATE INDEX IF NOT EXISTS idx_episodic_source ON episodic_memories(source_label)",
    "CREATE INDEX IF NOT EXISTS idx_episodic_created ON episodic_memories(created_at)",
    "CREATE INDEX IF NOT EXISTS idx_similar_source ON similar_to_edges(source_id)",
    "CREATE INDEX IF NOT EXISTS idx_similar_target ON similar_to_edges(target_id)",
    "CREATE INDEX IF NOT EXISTS idx_derives_source ON derives_from_edges(source_id)",
    "CREATE INDEX IF NOT EXISTS idx_derives_target ON derives_from_edges(target_id)",
    "CREATE INDEX IF NOT EXISTS idx_supersedes_source ON supersedes_edges(source_id)",
    "CREATE INDEX IF NOT EXISTS idx_supersedes_target ON supersedes_edges(target_id)",
    "CREATE INDEX IF NOT EXISTS idx_transitioned_source ON transitioned_to_edges(source_id)",
    "CREATE INDEX IF NOT EXISTS idx_transitioned_target ON transitioned_to_edges(target_id)",
]);

/// Initialise the hierarchical SQLite schema (tables + indexes).
///
/// Uses `CREATE TABLE IF NOT EXISTS` and `CREATE INDEX IF NOT EXISTS` so this
/// function is idempotent.
pub(crate) fn init_hierarchical_sqlite_schema(conn: &SqliteConnection) -> Result<()> {
    conn.execute_batch(SQLITE_HIERARCHICAL_SCHEMA)
        .context("failed to create hierarchical SQLite tables")?;
    for statement in SQLITE_HIERARCHICAL_INDEXES.iter() {
        conn.execute_batch(statement)
            .with_context(|| format!("failed to create index: {statement}"))?;
    }
    Ok(())
}
