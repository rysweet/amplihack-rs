//! SQLite hierarchical transfer backend.
//!
//! Implements `HierarchicalTransferBackend` for SQLite, mirroring the Kuzu
//! implementation semantics so both backends are interchangeable for
//! export/import workflows.

use super::super::parse_json_value;
use super::backend::HierarchicalTransferBackend;
use super::{
    DerivesEdge, EpisodicNode, ExportResult, HierarchicalExportData, HierarchicalStats,
    ImportResult, ImportStats, SemanticNode, SimilarEdge, SupersedesEdge, TransitionEdge,
    kuzu_export_timestamp,
};
use anyhow::{Context, Result};
use rusqlite::{Connection as SqliteConnection, params};
use serde_json::Value as JsonValue;
use std::fs;
use std::io::Read;
use std::path::PathBuf;

/// Maximum allowed JSON file size: 500 MiB.
const MAX_JSON_FILE_SIZE: u64 = 500 * 1024 * 1024;

/// Maximum agent name length (filesystem constraint).
const MAX_AGENT_NAME_LEN: usize = 255;

/// CREATE TABLE statements for the hierarchical SQLite schema.
///
/// Six tables matching the Kuzu node/rel tables:
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

/// Validate an agent name, rejecting path traversal and invalid names.
///
/// Rules:
/// - Must not be empty.
/// - Must not exceed `MAX_AGENT_NAME_LEN` characters.
/// - Must not contain `..` (path traversal component).
/// - Must not be an absolute path (start with `/`).
pub(crate) fn validate_agent_name(agent_name: &str) -> Result<()> {
    if agent_name.is_empty() {
        anyhow::bail!("agent name must not be empty");
    }
    if agent_name.len() > MAX_AGENT_NAME_LEN {
        anyhow::bail!(
            "agent name is too long ({} characters, max {MAX_AGENT_NAME_LEN})",
            agent_name.len()
        );
    }
    // Reject absolute paths.
    if agent_name.starts_with('/') {
        anyhow::bail!("agent name must not be an absolute path: {agent_name:?}");
    }
    // Reject path traversal components.
    let path = std::path::Path::new(agent_name);
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => {
                anyhow::bail!("agent name contains path traversal component '..': {agent_name:?}");
            }
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("agent name contains absolute path component: {agent_name:?}");
            }
            _ => {}
        }
    }
    Ok(())
}

/// Resolve the SQLite database file path for a given agent.
///
/// Validation order: `validate_agent_name` is called FIRST before any
/// `PathBuf` construction.
///
/// If `storage_path` is `Some(path)`, the database lives at
/// `<storage_path>/<agent_name>.db`.
/// If `storage_path` is `None`, the database lives at
/// `~/.amplihack/hierarchical_memory/<agent_name>.db`.
pub(crate) fn resolve_hierarchical_sqlite_path(
    agent_name: &str,
    storage_path: Option<&str>,
) -> Result<PathBuf> {
    validate_agent_name(agent_name)?;

    let base = match storage_path {
        Some(path) => PathBuf::from(path),
        None => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .context("HOME environment variable is not set")?;
            home.join(".amplihack").join("hierarchical_memory")
        }
    };

    Ok(base.join(format!("{agent_name}.db")))
}

/// Enforce restrictive filesystem permissions on a SQLite database file.
///
/// Sets file permissions to 0o600 (owner read/write only) and the parent
/// directory to 0o700 (owner access only).
#[cfg(unix)]
pub(crate) fn enforce_hierarchical_db_permissions(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)
            .with_context(|| format!("failed to set 0o600 on {}", path.display()))?;
    }
    if let Some(parent) = path.parent()
        && parent.exists()
    {
        let dir_perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(parent, dir_perms)
            .with_context(|| format!("failed to set 0o700 on {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn enforce_hierarchical_db_permissions(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

/// Open a SQLite connection to the hierarchical database and initialise the
/// schema.
fn open_hierarchical_sqlite_conn(db_path: &std::path::Path) -> Result<SqliteConnection> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let conn = SqliteConnection::open(db_path)
        .with_context(|| format!("failed to open SQLite at {}", db_path.display()))?;
    enforce_hierarchical_db_permissions(db_path)?;
    init_hierarchical_sqlite_schema(&conn)?;
    Ok(conn)
}

/// Stateless SQLite hierarchical transfer backend.
pub(crate) struct SqliteHierarchicalTransferBackend;

impl HierarchicalTransferBackend for SqliteHierarchicalTransferBackend {
    fn export_hierarchical_json(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult> {
        let db_path = resolve_hierarchical_sqlite_path(agent_name, storage_path)?;
        let conn = open_hierarchical_sqlite_conn(&db_path)?;

        let semantic_nodes = load_semantic_nodes(&conn, agent_name)?;
        let episodic_nodes = load_episodic_nodes(&conn, agent_name)?;
        let similar_to_edges = load_similar_to_edges(&conn, agent_name)?;
        let derives_from_edges = load_derives_from_edges(&conn, agent_name)?;
        let supersedes_edges = load_supersedes_edges(&conn, agent_name)?;
        let transitioned_to_edges = load_transitioned_to_edges(&conn, agent_name)?;

        let statistics = HierarchicalStats {
            semantic_node_count: semantic_nodes.len(),
            episodic_node_count: episodic_nodes.len(),
            similar_to_edge_count: similar_to_edges.len(),
            derives_from_edge_count: derives_from_edges.len(),
            supersedes_edge_count: supersedes_edges.len(),
            transitioned_to_edge_count: transitioned_to_edges.len(),
        };

        let export = HierarchicalExportData {
            agent_name: agent_name.to_string(),
            exported_at: kuzu_export_timestamp(),
            format_version: "1.1".to_string(),
            semantic_nodes,
            episodic_nodes,
            similar_to_edges,
            derives_from_edges,
            supersedes_edges,
            transitioned_to_edges,
            statistics,
        };

        let output_path = PathBuf::from(output);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write to a .tmp file, then atomically rename.
        let tmp_path = output_path.with_extension("json.tmp");
        let serialized = serde_json::to_string_pretty(&export)
            .context("failed to serialize hierarchical export data")?;
        fs::write(&tmp_path, &serialized)
            .with_context(|| format!("failed to write tmp file {}", tmp_path.display()))?;
        fs::rename(&tmp_path, &output_path)
            .with_context(|| format!("failed to rename tmp to {}", output_path.display()))?;

        let file_size = output_path.metadata()?.len();
        Ok(ExportResult {
            agent_name: agent_name.to_string(),
            format: "json".to_string(),
            output_path: output_path.canonicalize()?.display().to_string(),
            file_size_bytes: Some(file_size),
            statistics: vec![
                (
                    "semantic_node_count".to_string(),
                    export.statistics.semantic_node_count.to_string(),
                ),
                (
                    "episodic_node_count".to_string(),
                    export.statistics.episodic_node_count.to_string(),
                ),
                (
                    "similar_to_edge_count".to_string(),
                    export.statistics.similar_to_edge_count.to_string(),
                ),
                (
                    "derives_from_edge_count".to_string(),
                    export.statistics.derives_from_edge_count.to_string(),
                ),
                (
                    "supersedes_edge_count".to_string(),
                    export.statistics.supersedes_edge_count.to_string(),
                ),
                (
                    "transitioned_to_edge_count".to_string(),
                    export.statistics.transitioned_to_edge_count.to_string(),
                ),
            ],
        })
    }

    fn import_hierarchical_json(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult> {
        let input_path = PathBuf::from(input);

        // Validate file size before deserializing.
        let file_meta = input_path
            .metadata()
            .with_context(|| format!("cannot stat input file {}", input_path.display()))?;
        if file_meta.len() > MAX_JSON_FILE_SIZE {
            anyhow::bail!(
                "input file exceeds maximum allowed size ({} bytes > {MAX_JSON_FILE_SIZE} bytes)",
                file_meta.len()
            );
        }

        let mut raw = String::new();
        fs::File::open(&input_path)
            .with_context(|| format!("cannot open {}", input_path.display()))?
            .read_to_string(&mut raw)?;
        let data: HierarchicalExportData =
            serde_json::from_str(&raw).context("failed to deserialize hierarchical export JSON")?;

        let db_path = resolve_hierarchical_sqlite_path(agent_name, storage_path)?;
        let conn = open_hierarchical_sqlite_conn(&db_path)?;

        if !merge {
            clear_agent_data(&conn, agent_name)?;
        }

        let existing_ids: std::collections::HashSet<String> = if merge {
            get_existing_ids(&conn, agent_name)?.into_iter().collect()
        } else {
            std::collections::HashSet::new()
        };

        let mut stats = ImportStats::default();

        for node in &data.episodic_nodes {
            if node.memory_id.is_empty() {
                stats.errors += 1;
                continue;
            }
            if merge && existing_ids.contains(&node.memory_id) {
                stats.skipped += 1;
                continue;
            }
            let result = conn.execute(
                "INSERT OR IGNORE INTO episodic_memories (memory_id, agent_id, content, source_label, tags, metadata, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    node.memory_id,
                    agent_name,
                    node.content,
                    node.source_label,
                    serde_json::to_string(&node.tags)?,
                    serde_json::to_string(&node.metadata)?,
                    node.created_at,
                ],
            );
            match result {
                Ok(_) => stats.episodic_nodes_imported += 1,
                Err(_) => stats.errors += 1,
            }
        }

        for node in &data.semantic_nodes {
            if node.memory_id.is_empty() {
                stats.errors += 1;
                continue;
            }
            if merge && existing_ids.contains(&node.memory_id) {
                stats.skipped += 1;
                continue;
            }
            let result = conn.execute(
                "INSERT OR IGNORE INTO semantic_memories (memory_id, agent_id, concept, content, confidence, source_id, tags, metadata, created_at, entity_name) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    node.memory_id,
                    agent_name,
                    node.concept,
                    node.content,
                    node.confidence,
                    node.source_id,
                    serde_json::to_string(&node.tags)?,
                    serde_json::to_string(&node.metadata)?,
                    node.created_at,
                    node.entity_name,
                ],
            );
            match result {
                Ok(_) => stats.semantic_nodes_imported += 1,
                Err(_) => stats.errors += 1,
            }
        }

        for edge in &data.similar_to_edges {
            let result = conn.execute(
                "INSERT INTO similar_to_edges (source_id, target_id, weight, metadata) VALUES (?1, ?2, ?3, ?4)",
                params![
                    edge.source_id,
                    edge.target_id,
                    edge.weight,
                    serde_json::to_string(&edge.metadata)?,
                ],
            );
            match result {
                Ok(_) => stats.edges_imported += 1,
                Err(_) => stats.errors += 1,
            }
        }

        for edge in &data.derives_from_edges {
            let result = conn.execute(
                "INSERT INTO derives_from_edges (source_id, target_id, extraction_method, confidence) VALUES (?1, ?2, ?3, ?4)",
                params![
                    edge.source_id,
                    edge.target_id,
                    edge.extraction_method,
                    edge.confidence,
                ],
            );
            match result {
                Ok(_) => stats.edges_imported += 1,
                Err(_) => stats.errors += 1,
            }
        }

        for edge in &data.supersedes_edges {
            let result = conn.execute(
                "INSERT INTO supersedes_edges (source_id, target_id, reason, temporal_delta) VALUES (?1, ?2, ?3, ?4)",
                params![
                    edge.source_id,
                    edge.target_id,
                    edge.reason,
                    edge.temporal_delta,
                ],
            );
            match result {
                Ok(_) => stats.edges_imported += 1,
                Err(_) => stats.errors += 1,
            }
        }

        for edge in &data.transitioned_to_edges {
            let result = conn.execute(
                "INSERT INTO transitioned_to_edges (source_id, target_id, from_value, to_value, turn, transition_type) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    edge.source_id,
                    edge.target_id,
                    edge.from_value,
                    edge.to_value,
                    edge.turn,
                    edge.transition_type,
                ],
            );
            match result {
                Ok(_) => stats.edges_imported += 1,
                Err(_) => stats.errors += 1,
            }
        }

        Ok(ImportResult {
            agent_name: agent_name.to_string(),
            format: "json".to_string(),
            source_agent: Some(data.agent_name),
            merge,
            statistics: vec![
                (
                    "semantic_nodes_imported".to_string(),
                    stats.semantic_nodes_imported.to_string(),
                ),
                (
                    "episodic_nodes_imported".to_string(),
                    stats.episodic_nodes_imported.to_string(),
                ),
                (
                    "edges_imported".to_string(),
                    stats.edges_imported.to_string(),
                ),
                ("skipped".to_string(), stats.skipped.to_string()),
                ("errors".to_string(), stats.errors.to_string()),
            ],
        })
    }

    fn export_hierarchical_raw_db(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult> {
        let db_path = resolve_hierarchical_sqlite_path(agent_name, storage_path)?;
        let output_path = PathBuf::from(output);

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Remove existing output.
        if output_path.symlink_metadata().is_ok() {
            if output_path.is_dir() {
                fs::remove_dir_all(&output_path)?;
            } else {
                fs::remove_file(&output_path)?;
            }
        }

        // If the source DB exists, copy it; otherwise create an empty output.
        if db_path.symlink_metadata().is_ok() {
            if db_path.is_symlink() {
                anyhow::bail!(
                    "source database is a symlink, refusing to copy: {}",
                    db_path.display()
                );
            }
            fs::copy(&db_path, &output_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    db_path.display(),
                    output_path.display()
                )
            })?;
        } else {
            // Create a new empty DB at the output location.
            let conn = SqliteConnection::open(&output_path)?;
            init_hierarchical_sqlite_schema(&conn)?;
        }

        let file_size = output_path.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(ExportResult {
            agent_name: agent_name.to_string(),
            format: "raw-db".to_string(),
            output_path: output_path.canonicalize()?.display().to_string(),
            file_size_bytes: Some(file_size),
            statistics: vec![(
                "note".to_string(),
                "Raw SQLite DB copy - use JSON format for node/edge counts".to_string(),
            )],
        })
    }

    fn import_hierarchical_raw_db(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult> {
        if merge {
            anyhow::bail!(
                "Merge mode is not supported for raw-db format. Use JSON format for merge imports, or set merge=false to replace the DB entirely."
            );
        }

        let input_path = PathBuf::from(input);
        if input_path.symlink_metadata().is_err() {
            anyhow::bail!("input path does not exist: {}", input_path.display());
        }

        let target_path = resolve_hierarchical_sqlite_path(agent_name, storage_path)?;
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Backup existing target if present.
        if target_path.symlink_metadata().is_ok() {
            let backup_path = target_path.with_extension("db.bak");
            if backup_path.symlink_metadata().is_ok() {
                if backup_path.is_dir() {
                    fs::remove_dir_all(&backup_path)?;
                } else {
                    fs::remove_file(&backup_path)?;
                }
            }
            fs::rename(&target_path, &backup_path)?;
        }

        if input_path.is_dir() {
            // If input is a directory (unlikely for sqlite but handle gracefully),
            // just copy the directory.
            copy_dir(&input_path, &target_path)?;
        } else {
            fs::copy(&input_path, &target_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    input_path.display(),
                    target_path.display()
                )
            })?;
        }

        Ok(ImportResult {
            agent_name: agent_name.to_string(),
            format: "raw-db".to_string(),
            source_agent: None,
            merge: false,
            statistics: vec![(
                "note".to_string(),
                "Raw SQLite DB replaced - restart agent to use new DB".to_string(),
            )],
        })
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn load_semantic_nodes(conn: &SqliteConnection, agent_name: &str) -> Result<Vec<SemanticNode>> {
    let mut stmt = conn.prepare(
        "SELECT memory_id, concept, content, confidence, source_id, tags, metadata, created_at, entity_name FROM semantic_memories WHERE agent_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load semantic nodes")?;

    rows.into_iter()
        .map(
            |(
                memory_id,
                concept,
                content,
                confidence,
                source_id,
                tags_raw,
                metadata_raw,
                created_at,
                entity_name,
            )| {
                Ok(SemanticNode {
                    memory_id,
                    concept,
                    content,
                    confidence,
                    source_id,
                    tags: serde_json::from_str(&tags_raw).unwrap_or_default(),
                    metadata: parse_json_value(&metadata_raw)
                        .unwrap_or(JsonValue::Object(Default::default())),
                    created_at,
                    entity_name,
                })
            },
        )
        .collect()
}

fn load_episodic_nodes(conn: &SqliteConnection, agent_name: &str) -> Result<Vec<EpisodicNode>> {
    let mut stmt = conn.prepare(
        "SELECT memory_id, content, source_label, tags, metadata, created_at FROM episodic_memories WHERE agent_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load episodic nodes")?;

    rows.into_iter()
        .map(
            |(memory_id, content, source_label, tags_raw, metadata_raw, created_at)| {
                Ok(EpisodicNode {
                    memory_id,
                    content,
                    source_label,
                    tags: serde_json::from_str(&tags_raw).unwrap_or_default(),
                    metadata: parse_json_value(&metadata_raw)
                        .unwrap_or(JsonValue::Object(Default::default())),
                    created_at,
                })
            },
        )
        .collect()
}

fn load_similar_to_edges(conn: &SqliteConnection, agent_name: &str) -> Result<Vec<SimilarEdge>> {
    let mut stmt = conn.prepare(
        "SELECT s.source_id, s.target_id, s.weight, s.metadata FROM similar_to_edges s JOIN semantic_memories sm ON s.source_id = sm.memory_id WHERE sm.agent_id = ?1",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load similar_to edges")?;

    rows.into_iter()
        .map(|(source_id, target_id, weight, metadata_raw)| {
            Ok(SimilarEdge {
                source_id,
                target_id,
                weight,
                metadata: parse_json_value(&metadata_raw)
                    .unwrap_or(JsonValue::Object(Default::default())),
            })
        })
        .collect()
}

fn load_derives_from_edges(conn: &SqliteConnection, agent_name: &str) -> Result<Vec<DerivesEdge>> {
    let mut stmt = conn.prepare(
        "SELECT d.source_id, d.target_id, d.extraction_method, d.confidence FROM derives_from_edges d JOIN semantic_memories sm ON d.source_id = sm.memory_id WHERE sm.agent_id = ?1",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load derives_from edges")?;

    rows.into_iter()
        .map(|(source_id, target_id, extraction_method, confidence)| {
            Ok(DerivesEdge {
                source_id,
                target_id,
                extraction_method,
                confidence,
            })
        })
        .collect()
}

fn load_supersedes_edges(conn: &SqliteConnection, agent_name: &str) -> Result<Vec<SupersedesEdge>> {
    let mut stmt = conn.prepare(
        "SELECT s.source_id, s.target_id, s.reason, s.temporal_delta FROM supersedes_edges s JOIN semantic_memories sm ON s.source_id = sm.memory_id WHERE sm.agent_id = ?1",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load supersedes edges")?;

    rows.into_iter()
        .map(|(source_id, target_id, reason, temporal_delta)| {
            Ok(SupersedesEdge {
                source_id,
                target_id,
                reason,
                temporal_delta,
            })
        })
        .collect()
}

fn load_transitioned_to_edges(
    conn: &SqliteConnection,
    agent_name: &str,
) -> Result<Vec<TransitionEdge>> {
    let mut stmt = conn.prepare(
        "SELECT t.source_id, t.target_id, t.from_value, t.to_value, t.turn, t.transition_type FROM transitioned_to_edges t JOIN semantic_memories sm ON t.source_id = sm.memory_id WHERE sm.agent_id = ?1",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load transitioned_to edges")?;

    rows.into_iter()
        .map(
            |(source_id, target_id, from_value, to_value, turn, transition_type)| {
                Ok(TransitionEdge {
                    source_id,
                    target_id,
                    from_value,
                    to_value,
                    turn,
                    transition_type,
                })
            },
        )
        .collect()
}

fn clear_agent_data(conn: &SqliteConnection, agent_name: &str) -> Result<()> {
    // Delete edges whose source node belongs to this agent.
    conn.execute(
        "DELETE FROM similar_to_edges WHERE source_id IN (SELECT memory_id FROM semantic_memories WHERE agent_id = ?1)",
        params![agent_name],
    )?;
    conn.execute(
        "DELETE FROM derives_from_edges WHERE source_id IN (SELECT memory_id FROM semantic_memories WHERE agent_id = ?1)",
        params![agent_name],
    )?;
    conn.execute(
        "DELETE FROM supersedes_edges WHERE source_id IN (SELECT memory_id FROM semantic_memories WHERE agent_id = ?1)",
        params![agent_name],
    )?;
    conn.execute(
        "DELETE FROM transitioned_to_edges WHERE source_id IN (SELECT memory_id FROM semantic_memories WHERE agent_id = ?1)",
        params![agent_name],
    )?;
    conn.execute(
        "DELETE FROM semantic_memories WHERE agent_id = ?1",
        params![agent_name],
    )?;
    conn.execute(
        "DELETE FROM episodic_memories WHERE agent_id = ?1",
        params![agent_name],
    )?;
    Ok(())
}

fn get_existing_ids(conn: &SqliteConnection, agent_name: &str) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let mut stmt = conn.prepare("SELECT memory_id FROM semantic_memories WHERE agent_id = ?1")?;
    let rows = stmt
        .query_map(params![agent_name], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.extend(rows);

    let mut stmt2 = conn.prepare("SELECT memory_id FROM episodic_memories WHERE agent_id = ?1")?;
    let rows2 = stmt2
        .query_map(params![agent_name], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.extend(rows2);
    Ok(ids)
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
