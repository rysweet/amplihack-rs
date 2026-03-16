//! Native memory commands (`tree`, `export`, `import`, `clean`).

pub mod backend;
pub mod clean;
pub mod code_graph;
pub mod indexing_job;
pub mod scip_indexing;
pub mod staleness_detector;
pub mod transfer;
pub mod tree;

pub use clean::run_clean;
pub use code_graph::{
    CodeGraphSummary, default_code_graph_db_path_for_project, import_scip_file,
    resolve_code_graph_db_path_for_project, run_index_code, summarize_code_graph,
};
pub use indexing_job::{
    background_index_job_active, background_index_job_path, record_background_index_pid,
};
pub use scip_indexing::{
    check_prerequisites, detect_project_languages, run_index_scip, run_native_scip_indexing,
};
pub use staleness_detector::{IndexStatus, check_index_status};
pub use transfer::{run_export, run_import};
pub use tree::run_tree;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use kuzu::{
    Connection as KuzuConnection, Database as KuzuDatabase, SystemConfig, Value as KuzuValue,
};
use rusqlite::{Connection as SqliteConnection, params};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use time::OffsetDateTime;

pub(crate) const SQLITE_TREE_BACKEND_NAME: &str = "unknown";
pub(crate) const KUZU_TREE_BACKEND_NAME: &str = "kuzu";
pub(crate) const SQLITE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS memory_entries (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    memory_type TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT,
    metadata TEXT NOT NULL DEFAULT '{}',
    tags TEXT DEFAULT NULL,
    importance INTEGER DEFAULT NULL,
    created_at TEXT NOT NULL,
    accessed_at TEXT NOT NULL,
    expires_at TEXT DEFAULT NULL,
    parent_id TEXT DEFAULT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    last_accessed TEXT NOT NULL,
    metadata TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS session_agents (
    session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    first_used TEXT NOT NULL,
    last_used TEXT NOT NULL,
    PRIMARY KEY (session_id, agent_id)
);
"#;
pub(crate) const HIERARCHICAL_SCHEMA: &[&str] = &[
    r#"CREATE NODE TABLE IF NOT EXISTS SemanticMemory(
        memory_id STRING,
        concept STRING,
        content STRING,
        confidence DOUBLE,
        source_id STRING,
        agent_id STRING,
        tags STRING,
        metadata STRING,
        created_at STRING,
        entity_name STRING DEFAULT '',
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS EpisodicMemory(
        memory_id STRING,
        content STRING,
        source_label STRING,
        agent_id STRING,
        tags STRING,
        metadata STRING,
        created_at STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS SIMILAR_TO(
        FROM SemanticMemory TO SemanticMemory,
        weight DOUBLE,
        metadata STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS DERIVES_FROM(
        FROM SemanticMemory TO EpisodicMemory,
        extraction_method STRING,
        confidence DOUBLE
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS SUPERSEDES(
        FROM SemanticMemory TO SemanticMemory,
        reason STRING,
        temporal_delta STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS TRANSITIONED_TO(
        FROM SemanticMemory TO SemanticMemory,
        from_value STRING,
        to_value STRING,
        turn INT64,
        transition_type STRING
    )"#,
];
pub(crate) const KUZU_BACKEND_SCHEMA: &[&str] = &[
    r#"CREATE NODE TABLE IF NOT EXISTS Session(
        session_id STRING,
        start_time TIMESTAMP,
        end_time TIMESTAMP,
        user_id STRING,
        context STRING,
        status STRING,
        created_at TIMESTAMP,
        last_accessed TIMESTAMP,
        metadata STRING,
        PRIMARY KEY (session_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS Agent(
        agent_id STRING,
        name STRING,
        first_used TIMESTAMP,
        last_used TIMESTAMP,
        PRIMARY KEY (agent_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS EpisodicMemory(
        memory_id STRING,
        timestamp TIMESTAMP,
        content STRING,
        event_type STRING,
        emotional_valence DOUBLE,
        importance_score DOUBLE,
        title STRING,
        metadata STRING,
        tags STRING,
        created_at TIMESTAMP,
        accessed_at TIMESTAMP,
        expires_at TIMESTAMP,
        agent_id STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS SemanticMemory(
        memory_id STRING,
        concept STRING,
        content STRING,
        category STRING,
        confidence_score DOUBLE,
        last_updated TIMESTAMP,
        version INT64,
        title STRING,
        metadata STRING,
        tags STRING,
        created_at TIMESTAMP,
        accessed_at TIMESTAMP,
        agent_id STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS ProceduralMemory(
        memory_id STRING,
        procedure_name STRING,
        description STRING,
        steps STRING,
        preconditions STRING,
        postconditions STRING,
        success_rate DOUBLE,
        usage_count INT64,
        last_used TIMESTAMP,
        title STRING,
        content STRING,
        metadata STRING,
        tags STRING,
        created_at TIMESTAMP,
        accessed_at TIMESTAMP,
        agent_id STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS ProspectiveMemory(
        memory_id STRING,
        intention STRING,
        trigger_condition STRING,
        priority STRING,
        due_date TIMESTAMP,
        status STRING,
        scope STRING,
        completion_criteria STRING,
        title STRING,
        content STRING,
        metadata STRING,
        tags STRING,
        created_at TIMESTAMP,
        accessed_at TIMESTAMP,
        expires_at TIMESTAMP,
        agent_id STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS WorkingMemory(
        memory_id STRING,
        content STRING,
        memory_type STRING,
        priority INT64,
        created_at TIMESTAMP,
        ttl_seconds INT64,
        title STRING,
        metadata STRING,
        tags STRING,
        accessed_at TIMESTAMP,
        expires_at TIMESTAMP,
        agent_id STRING,
        PRIMARY KEY (memory_id)
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS CONTAINS_EPISODIC(FROM Session TO EpisodicMemory, sequence_number INT64)"#,
    r#"CREATE REL TABLE IF NOT EXISTS CONTAINS_WORKING(FROM Session TO WorkingMemory, activation_level DOUBLE)"#,
    r#"CREATE REL TABLE IF NOT EXISTS CONTRIBUTES_TO_SEMANTIC(FROM Session TO SemanticMemory, contribution_type STRING, timestamp TIMESTAMP, delta STRING)"#,
    r#"CREATE REL TABLE IF NOT EXISTS USES_PROCEDURE(FROM Session TO ProceduralMemory, timestamp TIMESTAMP, success BOOL, notes STRING)"#,
    r#"CREATE REL TABLE IF NOT EXISTS CREATES_INTENTION(FROM Session TO ProspectiveMemory, timestamp TIMESTAMP)"#,
];
pub(crate) const KUZU_MEMORY_TABLES: &[(&str, &str)] = &[
    ("EpisodicMemory", "CONTAINS_EPISODIC"),
    ("SemanticMemory", "CONTRIBUTES_TO_SEMANTIC"),
    ("ProceduralMemory", "USES_PROCEDURE"),
    ("ProspectiveMemory", "CREATES_INTENTION"),
    ("WorkingMemory", "CONTAINS_WORKING"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendChoice {
    Kuzu,
    Sqlite,
}

impl BackendChoice {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "kuzu" => Ok(Self::Kuzu),
            "sqlite" => Ok(Self::Sqlite),
            other => anyhow::bail!("Invalid backend: {other}. Must be kuzu or sqlite"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferFormat {
    Json,
    Kuzu,
}

impl TransferFormat {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "json" => Ok(Self::Json),
            "kuzu" => Ok(Self::Kuzu),
            other => anyhow::bail!("Unsupported format: {other:?}. Use one of: ('json', 'kuzu')"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub memory_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryRecord {
    pub(crate) memory_id: String,
    pub(crate) memory_type: String,
    pub(crate) title: String,
    pub(crate) content: String,
    pub(crate) metadata: JsonValue,
    pub(crate) importance: Option<i64>,
    pub(crate) accessed_at: Option<String>,
    pub(crate) expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContextMemory {
    pub content: String,
    pub code_context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectedPromptContextMemory {
    memory_id: String,
    content: String,
    code_context: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionLearningRecord {
    session_id: String,
    agent_id: String,
    content: String,
    title: String,
    metadata: JsonValue,
    importance: i64,
}

pub(crate) fn open_sqlite_memory_db() -> Result<SqliteConnection> {
    let path = home_dir()?.join(".amplihack").join("memory.db");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let conn = SqliteConnection::open(path)?;
    conn.execute_batch(SQLITE_SCHEMA)?;
    Ok(conn)
}

pub(crate) fn list_sqlite_sessions_from_conn(
    conn: &SqliteConnection,
) -> Result<Vec<SessionSummary>> {
    let mut stmt = conn.prepare("SELECT session_id FROM sessions ORDER BY last_accessed DESC")?;
    let mut rows = stmt.query([])?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next()? {
        let session_id: String = row.get(0)?;
        let memory_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memory_entries WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        sessions.push(SessionSummary {
            session_id,
            memory_count: memory_count as usize,
        });
    }
    Ok(sessions)
}

pub(crate) fn query_sqlite_memories_for_session(
    conn: &SqliteConnection,
    session_id: &str,
    memory_type: Option<&str>,
) -> Result<Vec<MemoryRecord>> {
    let mut sql = String::from(
        "SELECT id, memory_type, title, content, metadata, importance, accessed_at, expires_at FROM memory_entries WHERE session_id = ?1 AND (expires_at IS NULL OR expires_at > datetime('now'))",
    );
    if memory_type.is_some() {
        sql.push_str(" AND memory_type = ?2");
    }
    sql.push_str(" ORDER BY accessed_at DESC, importance DESC");
    let mut stmt = conn.prepare(&sql)?;
    let mapper = |row: &rusqlite::Row<'_>| -> rusqlite::Result<MemoryRecord> {
        let metadata_raw: Option<String> = row.get(4)?;
        Ok(MemoryRecord {
            memory_id: row.get(0)?,
            memory_type: row.get(1)?,
            title: row.get(2)?,
            content: row.get(3)?,
            metadata: metadata_raw
                .as_deref()
                .map(parse_json_value)
                .transpose()
                .map_err(to_sqlite_err)?
                .unwrap_or(JsonValue::Object(Default::default())),
            importance: row.get(5)?,
            accessed_at: row.get(6)?,
            expires_at: row.get(7)?,
        })
    };
    let rows = if let Some(memory_type) = memory_type {
        stmt.query_map(params![session_id, memory_type], mapper)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map(params![session_id], mapper)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

pub(crate) fn collect_sqlite_agent_counts(conn: &SqliteConnection) -> Result<Vec<(String, usize)>> {
    let mut stmt = conn.prepare(
        "SELECT agent_id, COUNT(*) FROM memory_entries WHERE expires_at IS NULL OR expires_at > datetime('now') GROUP BY agent_id ORDER BY agent_id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub(crate) fn delete_sqlite_session(session_id: &str) -> Result<bool> {
    let conn = open_sqlite_memory_db()?;
    conn.execute(
        "DELETE FROM memory_entries WHERE session_id = ?1",
        params![session_id],
    )?;
    conn.execute(
        "DELETE FROM session_agents WHERE session_id = ?1",
        params![session_id],
    )?;
    let deleted = conn.execute(
        "DELETE FROM sessions WHERE session_id = ?1",
        params![session_id],
    )?;
    Ok(deleted > 0)
}

pub(crate) fn open_kuzu_memory_db() -> Result<KuzuDatabase> {
    let path = resolve_kuzu_memory_db_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(KuzuDatabase::new(path, SystemConfig::default())?)
}

pub(crate) fn resolve_kuzu_memory_db_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH")
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("AMPLIHACK_KUZU_DB_PATH")
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }

    Ok(home_dir()?.join(".amplihack").join("memory_kuzu.db"))
}

pub fn init_kuzu_backend_schema(conn: &KuzuConnection<'_>) -> Result<()> {
    for statement in KUZU_BACKEND_SCHEMA {
        conn.query(statement)?;
    }
    Ok(())
}

pub fn list_kuzu_sessions_from_conn(conn: &KuzuConnection<'_>) -> Result<Vec<SessionSummary>> {
    let rows = kuzu_rows(
        conn,
        "MATCH (s:Session) RETURN s.session_id, s.created_at, s.last_accessed, s.metadata ORDER BY s.last_accessed DESC",
        vec![],
    )?;
    let mut sessions = Vec::new();
    for row in rows {
        let session_id = kuzu_string(row.first())?;
        let memories = query_kuzu_memories_for_session(conn, &session_id)?;
        sessions.push(SessionSummary {
            session_id,
            memory_count: memories.len(),
        });
    }
    Ok(sessions)
}

pub(crate) fn query_kuzu_memories_for_session(
    conn: &KuzuConnection<'_>,
    session_id: &str,
) -> Result<Vec<MemoryRecord>> {
    let mut memories = Vec::new();
    for (label, rel_name) in KUZU_MEMORY_TABLES {
        let query = format!(
            "MATCH (s:Session {{session_id: $session_id}})-[:{rel_name}]->(m:{label}) RETURN m ORDER BY m.accessed_at DESC"
        );
        let rows = kuzu_rows(
            conn,
            &query,
            vec![("session_id", KuzuValue::String(session_id.to_string()))],
        )?;
        for row in rows {
            if let Some(value) = row.first() {
                memories.push(memory_from_kuzu_node(value, session_id, label)?);
            }
        }
    }
    Ok(memories)
}

pub(crate) fn collect_kuzu_agent_counts(conn: &KuzuConnection<'_>) -> Result<Vec<(String, usize)>> {
    // One bulk query per memory type returns (agent_id, count) for ALL agents at
    // once.  This reduces from O(5n) round-trips (5 per-agent COUNT queries) to
    // O(5) total — a constant number of DB calls regardless of agent count.
    let mut totals: HashMap<String, usize> = HashMap::new();
    for (label, _) in KUZU_MEMORY_TABLES {
        let rows = kuzu_rows(
            conn,
            &format!("MATCH (m:{label}) RETURN m.agent_id, COUNT(m)"),
            vec![],
        )?;
        for row in rows {
            let agent_id = kuzu_string(row.first())?;
            let count = kuzu_i64(row.get(1))? as usize;
            *totals.entry(agent_id).or_insert(0) += count;
        }
    }

    // Filter zero-count agents and restore the original ascending sort order.
    let mut counts: Vec<(String, usize)> =
        totals.into_iter().filter(|(_, total)| *total > 0).collect();
    counts.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(counts)
}

pub(crate) fn delete_kuzu_session(session_id: &str) -> Result<bool> {
    let db = open_kuzu_memory_db()?;
    let conn = KuzuConnection::new(&db)?;
    init_kuzu_backend_schema(&conn)?;
    let exists = kuzu_rows(
        &conn,
        "MATCH (s:Session {session_id: $session_id}) RETURN COUNT(s)",
        vec![("session_id", KuzuValue::String(session_id.to_string()))],
    )?;
    let existing = exists
        .first()
        .map(|row| kuzu_i64(row.first()).unwrap_or(0))
        .unwrap_or(0);
    if existing == 0 {
        return Ok(false);
    }
    for (label, rel_name) in KUZU_MEMORY_TABLES {
        let query = format!(
            "MATCH (s:Session {{session_id: $session_id}})-[:{rel_name}]->(m:{label}) DETACH DELETE m"
        );
        kuzu_rows(
            &conn,
            &query,
            vec![("session_id", KuzuValue::String(session_id.to_string()))],
        )?;
    }
    kuzu_rows(
        &conn,
        "MATCH (s:Session {session_id: $session_id}) DETACH DELETE s",
        vec![("session_id", KuzuValue::String(session_id.to_string()))],
    )?;
    Ok(true)
}

pub fn kuzu_rows(
    conn: &KuzuConnection<'_>,
    query: &str,
    params: Vec<(&str, KuzuValue)>,
) -> Result<Vec<Vec<KuzuValue>>> {
    if params.is_empty() {
        return Ok(conn.query(query)?.collect());
    }
    let mut prepared = conn.prepare(query)?;
    Ok(conn.execute(&mut prepared, params)?.collect())
}

pub(crate) fn memory_from_kuzu_node(
    value: &KuzuValue,
    _session_id: &str,
    label: &str,
) -> Result<MemoryRecord> {
    let props = match value {
        KuzuValue::Node(node) => node.get_properties(),
        other => anyhow::bail!("expected Kùzu node, got {other}"),
    };
    let metadata = property_string(props, "metadata")
        .as_deref()
        .map(parse_json_value)
        .transpose()?
        .unwrap_or(JsonValue::Object(Default::default()));
    let importance = property_i64(props, "importance")
        .or_else(|| property_i64(props, "importance_score"))
        .or_else(|| property_i64(props, "priority"));
    Ok(MemoryRecord {
        memory_id: property_string(props, "memory_id").unwrap_or_default(),
        memory_type: label
            .strip_suffix("Memory")
            .unwrap_or(label)
            .to_ascii_lowercase(),
        title: property_string(props, "title")
            .or_else(|| property_string(props, "concept"))
            .or_else(|| property_string(props, "procedure_name"))
            .unwrap_or_default(),
        content: property_string(props, "content").unwrap_or_default(),
        metadata,
        importance,
        accessed_at: property_string(props, "accessed_at"),
        expires_at: property_string(props, "expires_at"),
    })
}

pub(crate) fn property_string(props: &[(String, KuzuValue)], key: &str) -> Option<String> {
    props.iter().find_map(|(name, value)| {
        if name == key {
            Some(kuzu_value_to_string(value))
        } else {
            None
        }
    })
}

pub(crate) fn property_i64(props: &[(String, KuzuValue)], key: &str) -> Option<i64> {
    props.iter().find_map(|(name, value)| {
        if name == key {
            kuzu_value_to_i64(value)
        } else {
            None
        }
    })
}

pub(crate) fn kuzu_value_to_string(value: &KuzuValue) -> String {
    match value {
        KuzuValue::Null(_) => String::new(),
        KuzuValue::String(v) => v.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn kuzu_value_to_i64(value: &KuzuValue) -> Option<i64> {
    match value {
        KuzuValue::Int64(v) => Some(*v),
        KuzuValue::Int32(v) => Some(i64::from(*v)),
        KuzuValue::Int16(v) => Some(i64::from(*v)),
        KuzuValue::Int8(v) => Some(i64::from(*v)),
        KuzuValue::UInt64(v) => i64::try_from(*v).ok(),
        KuzuValue::UInt32(v) => Some(i64::from(*v)),
        KuzuValue::UInt16(v) => Some(i64::from(*v)),
        KuzuValue::UInt8(v) => Some(i64::from(*v)),
        KuzuValue::Double(v) => Some(*v as i64),
        KuzuValue::Float(v) => Some(*v as i64),
        _ => None,
    }
}

pub(crate) fn kuzu_string(value: Option<&KuzuValue>) -> Result<String> {
    Ok(value.map(kuzu_value_to_string).unwrap_or_default())
}

pub(crate) fn kuzu_i64(value: Option<&KuzuValue>) -> Result<i64> {
    value
        .and_then(kuzu_value_to_i64)
        .context("expected integer Kùzu value")
}

pub(crate) fn kuzu_f64(value: Option<&KuzuValue>) -> Result<f64> {
    match value {
        Some(KuzuValue::Double(v)) => Ok(*v),
        Some(KuzuValue::Float(v)) => Ok(f64::from(*v)),
        Some(KuzuValue::Int64(v)) => Ok(*v as f64),
        Some(KuzuValue::Int32(v)) => Ok(f64::from(*v)),
        Some(KuzuValue::UInt64(v)) => Ok(*v as f64),
        Some(KuzuValue::UInt32(v)) => Ok(f64::from(*v)),
        Some(KuzuValue::Null(_)) | None => Ok(0.0),
        Some(other) => anyhow::bail!("expected numeric Kùzu value, got {other}"),
    }
}

pub(crate) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}

pub(crate) fn to_sqlite_err(error: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}

pub(crate) fn parse_json_value(value: &str) -> Result<JsonValue> {
    if value.is_empty() {
        return Ok(JsonValue::Object(Default::default()));
    }
    Ok(serde_json::from_str(value)?)
}

fn resolve_memory_backend_preference() -> Option<BackendChoice> {
    match std::env::var("AMPLIHACK_MEMORY_BACKEND").ok().as_deref() {
        Some("sqlite") => Some(BackendChoice::Sqlite),
        Some("kuzu") => Some(BackendChoice::Kuzu),
        _ => None,
    }
}

fn load_runtime_memories_from_backend(
    choice: BackendChoice,
    session_id: &str,
) -> Result<Vec<MemoryRecord>> {
    self::backend::open_runtime_backend(choice)?.load_prompt_context_memories(session_id)
}

fn is_prompt_context_memory(memory: &MemoryRecord) -> bool {
    matches!(
        memory
            .metadata
            .get("new_memory_type")
            .and_then(JsonValue::as_str),
        Some("episodic" | "semantic" | "procedural")
    )
}

fn parse_memory_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc())
        })
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc())
        })
}

/// Score a single memory record against a pre-lowercased query string.
///
/// `query_lower` **must** already be lowercase; callers are responsible for
/// converting once before iterating over many records (avoids O(n) repeated
/// allocations for the same query).
fn memory_relevance_score(memory: &MemoryRecord, query_lower: &str) -> f64 {
    let content_lower = memory.content.to_lowercase();
    let mut score = 0.0;

    if !query_lower.is_empty() && content_lower.contains(query_lower) {
        score += 10.0;
    }

    let query_words: HashSet<&str> = query_lower.split_whitespace().collect();
    let content_words: HashSet<&str> = content_lower.split_whitespace().collect();
    score += query_words.intersection(&content_words).count() as f64 * 2.0;

    if let Some(accessed_at) = memory.accessed_at.as_deref()
        && let Some(timestamp) = parse_memory_timestamp(accessed_at)
    {
        let age_days = (Utc::now() - timestamp).num_days().max(0) as f64;
        score += (5.0 - (age_days * 0.1)).max(0.0);
    }

    if let Some(importance) = memory.importance {
        score += importance as f64;
    }

    score
}

fn select_prompt_context_memories(
    memories: Vec<MemoryRecord>,
    query_text: &str,
    token_budget: usize,
) -> Vec<SelectedPromptContextMemory> {
    if token_budget == 0 {
        return Vec::new();
    }

    // Pre-compute once; `memory_relevance_score` expects a pre-lowercased string
    // so we don't re-allocate the lowercase form on every memory record.
    let query_lower = query_text.to_lowercase();

    let mut ranked = memories
        .into_iter()
        .filter(is_prompt_context_memory)
        .map(|memory| {
            let score = memory_relevance_score(&memory, &query_lower);
            (memory, score)
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));

    let mut total_tokens = 0usize;
    let mut selected = Vec::new();
    for (memory, _) in ranked {
        let memory_tokens = memory.content.chars().count() / 4;
        if total_tokens + memory_tokens > token_budget {
            break;
        }
        selected.push(SelectedPromptContextMemory {
            memory_id: memory.memory_id,
            content: memory.content,
            code_context: None,
        });
        total_tokens += memory_tokens;
    }

    selected
}

fn format_code_context(payload: &code_graph::CodeGraphContextPayload) -> Option<String> {
    if payload.files.is_empty() && payload.functions.is_empty() && payload.classes.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    if !payload.files.is_empty() {
        lines.push("**Related Files:**".to_string());
        for file in payload.files.iter().take(5) {
            lines.push(format!("- {} ({})", file.path, file.language));
        }
    }

    if !payload.functions.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("**Related Functions:**".to_string());
        for function in payload.functions.iter().take(5) {
            let signature = if function.signature.trim().is_empty() {
                function.name.as_str()
            } else {
                function.signature.as_str()
            };
            lines.push(format!("- `{}`", signature));
            if !function.docstring.trim().is_empty() {
                let doc_preview = if function.docstring.chars().count() > 100 {
                    let truncated = function.docstring.chars().take(100).collect::<String>();
                    format!("{truncated}...")
                } else {
                    function.docstring.clone()
                };
                lines.push(format!("  {doc_preview}"));
            }
            if function.complexity > 0 {
                lines.push(format!("  (complexity: {})", function.complexity));
            }
        }
    }

    if !payload.classes.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("**Related Classes:**".to_string());
        for class in payload.classes.iter().take(3) {
            let name = if class.fully_qualified_name.trim().is_empty() {
                class.name.as_str()
            } else {
                class.fully_qualified_name.as_str()
            };
            lines.push(format!("- {}", name));
            if !class.docstring.trim().is_empty() {
                let doc_preview = if class.docstring.chars().count() > 100 {
                    let truncated = class.docstring.chars().take(100).collect::<String>();
                    format!("{truncated}...")
                } else {
                    class.docstring.clone()
                };
                lines.push(format!("  {doc_preview}"));
            }
        }
    }

    Some(lines.join("\n"))
}

fn enrich_prompt_context_memories_with_code_context(
    selected: Vec<SelectedPromptContextMemory>,
) -> Result<Vec<SelectedPromptContextMemory>> {
    if selected.is_empty() {
        return Ok(selected);
    }

    let db_path = resolve_kuzu_memory_db_path()?;
    let reader = match code_graph::open_code_graph_reader(Some(&db_path)) {
        Ok(reader) => reader,
        Err(error) => {
            tracing::warn!(
                db_path = %db_path.display(),
                "prompt memory code-context enrichment unavailable: {}",
                error
            );
            return Ok(selected);
        }
    };

    let mut enriched = Vec::with_capacity(selected.len());
    for mut memory in selected {
        if memory.memory_id.trim().is_empty() {
            enriched.push(memory);
            continue;
        }

        match reader.context_payload(&memory.memory_id) {
            Ok(payload) => {
                memory.code_context = format_code_context(&payload);
            }
            Err(error) => {
                tracing::warn!(
                    memory_id = memory.memory_id,
                    "failed to load prompt memory code context: {}",
                    error
                );
            }
        }
        enriched.push(memory);
    }
    Ok(enriched)
}

fn retrieve_prompt_context_memories_from_backend(
    choice: BackendChoice,
    session_id: &str,
    query_text: &str,
    token_budget: usize,
) -> Result<Vec<PromptContextMemory>> {
    let memories = load_runtime_memories_from_backend(choice, session_id)?;
    let selected = select_prompt_context_memories(memories, query_text, token_budget);
    let selected = match choice {
        BackendChoice::Kuzu => enrich_prompt_context_memories_with_code_context(selected)?,
        BackendChoice::Sqlite => selected,
    };
    Ok(selected
        .into_iter()
        .map(|memory| PromptContextMemory {
            content: memory.content,
            code_context: memory.code_context,
        })
        .collect())
}

pub fn retrieve_prompt_context_memories(
    session_id: &str,
    query_text: &str,
    token_budget: usize,
) -> Result<Vec<PromptContextMemory>> {
    if session_id.trim().is_empty() || query_text.trim().is_empty() || token_budget == 0 {
        return Ok(Vec::new());
    }

    match resolve_memory_backend_preference() {
        Some(choice) => retrieve_prompt_context_memories_from_backend(
            choice,
            session_id,
            query_text,
            token_budget,
        ),
        None => retrieve_prompt_context_memories_from_backend(
            BackendChoice::Kuzu,
            session_id,
            query_text,
            token_budget,
        )
        .or_else(|_| {
            retrieve_prompt_context_memories_from_backend(
                BackendChoice::Sqlite,
                session_id,
                query_text,
                token_budget,
            )
        }),
    }
}

fn store_learning_sqlite(record: &SessionLearningRecord) -> Result<Option<String>> {
    let conn = open_sqlite_memory_db()?;
    let now = Utc::now().to_rfc3339();
    let memory_id = build_memory_id(record, &now);

    let duplicate_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_entries WHERE session_id = ?1 AND agent_id = ?2 AND content = ?3",
        params![record.session_id, record.agent_id, record.content],
        |row| row.get(0),
    )?;
    if duplicate_exists > 0 {
        return Ok(None);
    }

    conn.execute(
        "INSERT OR IGNORE INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
        params![record.session_id, now, now],
    )?;
    conn.execute(
        "UPDATE sessions SET last_accessed = ?2 WHERE session_id = ?1",
        params![record.session_id, now],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO session_agents (session_id, agent_id, first_used, last_used) VALUES (?1, ?2, ?3, ?4)",
        params![record.session_id, record.agent_id, now, now],
    )?;
    conn.execute(
        "UPDATE session_agents SET last_used = ?3 WHERE session_id = ?1 AND agent_id = ?2",
        params![record.session_id, record.agent_id, now],
    )?;
    conn.execute(
        "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            memory_id,
            record.session_id,
            record.agent_id,
            "learning",
            record.title,
            record.content,
            serde_json::to_string(&record.metadata)?,
            record.importance,
            now,
            now,
        ],
    )?;
    Ok(Some(memory_id))
}

fn store_learning_kuzu(record: &SessionLearningRecord) -> Result<Option<String>> {
    let db = open_kuzu_memory_db()?;
    let conn = KuzuConnection::new(&db)?;
    init_kuzu_backend_schema(&conn)?;

    let duplicate_rows = kuzu_rows(
        &conn,
        "MATCH (s:Session {session_id: $session_id})-[:CONTRIBUTES_TO_SEMANTIC]->(m:SemanticMemory) WHERE m.agent_id = $agent_id AND m.content = $content RETURN COUNT(m)",
        vec![
            ("session_id", KuzuValue::String(record.session_id.clone())),
            ("agent_id", KuzuValue::String(record.agent_id.clone())),
            ("content", KuzuValue::String(record.content.clone())),
        ],
    )?;
    let duplicate_count = duplicate_rows
        .first()
        .map(|row| kuzu_i64(row.first()).unwrap_or(0))
        .unwrap_or(0);
    if duplicate_count > 0 {
        return Ok(None);
    }

    let now = OffsetDateTime::now_utc();
    let now_str = Utc::now().to_rfc3339();
    let memory_id = build_memory_id(record, &now_str);
    let metadata = serde_json::to_string(&record.metadata)?;
    let tags = serde_json::to_string(&["learning", "session_end"])?;

    ensure_kuzu_session(&conn, &record.session_id, now)?;
    ensure_kuzu_agent(&conn, &record.agent_id, now)?;

    let mut create_memory = conn.prepare(
        "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, category: $category, confidence_score: $confidence_score, last_updated: $last_updated, version: $version, title: $title, metadata: $metadata, tags: $tags, created_at: $created_at, accessed_at: $accessed_at, agent_id: $agent_id})",
    )?;
    conn.execute(
        &mut create_memory,
        vec![
            ("memory_id", KuzuValue::String(memory_id.clone())),
            ("concept", KuzuValue::String(record.title.clone())),
            ("content", KuzuValue::String(record.content.clone())),
            ("category", KuzuValue::String("session_end".to_string())),
            ("confidence_score", KuzuValue::Double(1.0)),
            ("last_updated", KuzuValue::Timestamp(now)),
            ("version", KuzuValue::Int64(1)),
            ("title", KuzuValue::String(record.title.clone())),
            ("metadata", KuzuValue::String(metadata)),
            ("tags", KuzuValue::String(tags)),
            ("created_at", KuzuValue::Timestamp(now)),
            ("accessed_at", KuzuValue::Timestamp(now)),
            ("agent_id", KuzuValue::String(record.agent_id.clone())),
        ],
    )?;

    let mut create_link = conn.prepare(
        "MATCH (s:Session {session_id: $session_id}), (m:SemanticMemory {memory_id: $memory_id}) CREATE (s)-[:CONTRIBUTES_TO_SEMANTIC {contribution_type: $contribution_type, timestamp: $timestamp, delta: $delta}]->(m)",
    )?;
    conn.execute(
        &mut create_link,
        vec![
            ("session_id", KuzuValue::String(record.session_id.clone())),
            ("memory_id", KuzuValue::String(memory_id.clone())),
            (
                "contribution_type",
                KuzuValue::String("created".to_string()),
            ),
            ("timestamp", KuzuValue::Timestamp(now)),
            ("delta", KuzuValue::String("initial_creation".to_string())),
        ],
    )?;

    Ok(Some(memory_id))
}

fn ensure_kuzu_session(
    conn: &KuzuConnection<'_>,
    session_id: &str,
    now: OffsetDateTime,
) -> Result<()> {
    let count_rows = kuzu_rows(
        conn,
        "MATCH (s:Session {session_id: $session_id}) RETURN COUNT(s)",
        vec![("session_id", KuzuValue::String(session_id.to_string()))],
    )?;
    let count = count_rows
        .first()
        .map(|row| kuzu_i64(row.first()).unwrap_or(0))
        .unwrap_or(0);

    if count == 0 {
        let mut create = conn.prepare(
            "CREATE (s:Session {session_id: $session_id, start_time: $start_time, end_time: NULL, user_id: '', context: '', status: $status, created_at: $created_at, last_accessed: $last_accessed, metadata: $metadata})",
        )?;
        conn.execute(
            &mut create,
            vec![
                ("session_id", KuzuValue::String(session_id.to_string())),
                ("start_time", KuzuValue::Timestamp(now)),
                ("status", KuzuValue::String("active".to_string())),
                ("created_at", KuzuValue::Timestamp(now)),
                ("last_accessed", KuzuValue::Timestamp(now)),
                ("metadata", KuzuValue::String("{}".to_string())),
            ],
        )?;
    } else {
        let mut update = conn.prepare(
            "MATCH (s:Session {session_id: $session_id}) SET s.last_accessed = $last_accessed",
        )?;
        conn.execute(
            &mut update,
            vec![
                ("session_id", KuzuValue::String(session_id.to_string())),
                ("last_accessed", KuzuValue::Timestamp(now)),
            ],
        )?;
    }

    Ok(())
}

fn ensure_kuzu_agent(conn: &KuzuConnection<'_>, agent_id: &str, now: OffsetDateTime) -> Result<()> {
    let count_rows = kuzu_rows(
        conn,
        "MATCH (a:Agent {agent_id: $agent_id}) RETURN COUNT(a)",
        vec![("agent_id", KuzuValue::String(agent_id.to_string()))],
    )?;
    let count = count_rows
        .first()
        .map(|row| kuzu_i64(row.first()).unwrap_or(0))
        .unwrap_or(0);

    if count == 0 {
        let mut create = conn.prepare(
            "CREATE (a:Agent {agent_id: $agent_id, name: $name, first_used: $first_used, last_used: $last_used})",
        )?;
        conn.execute(
            &mut create,
            vec![
                ("agent_id", KuzuValue::String(agent_id.to_string())),
                ("name", KuzuValue::String(agent_id.to_string())),
                ("first_used", KuzuValue::Timestamp(now)),
                ("last_used", KuzuValue::Timestamp(now)),
            ],
        )?;
    } else {
        let mut update =
            conn.prepare("MATCH (a:Agent {agent_id: $agent_id}) SET a.last_used = $last_used")?;
        conn.execute(
            &mut update,
            vec![
                ("agent_id", KuzuValue::String(agent_id.to_string())),
                ("last_used", KuzuValue::Timestamp(now)),
            ],
        )?;
    }

    Ok(())
}

fn build_memory_id(record: &SessionLearningRecord, timestamp: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(record.session_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(record.agent_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(record.content.as_bytes());
    hasher.update(b"\0");
    hasher.update(timestamp.as_bytes());
    let digest = hasher.finalize();
    format!("mem-{:x}", digest)
}

fn heuristic_importance(content: &str) -> i64 {
    let len = content.trim().chars().count();
    match len {
        0..=99 => 5,
        100..=199 => 6,
        _ => 7,
    }
}

fn build_learning_record(
    session_id: &str,
    agent_id: &str,
    content: &str,
    task: Option<&str>,
    success: bool,
) -> Option<SessionLearningRecord> {
    let trimmed = content.trim();
    if trimmed.len() < 10 {
        return None;
    }

    let summary = trimmed.chars().take(500).collect::<String>();
    let title = summary.chars().take(50).collect::<String>();
    let project_id =
        std::env::var("AMPLIHACK_PROJECT_ID").unwrap_or_else(|_| "amplihack".to_string());
    Some(SessionLearningRecord {
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        content: format!("Agent {agent_id}: {summary}"),
        title: title.trim().to_string(),
        importance: heuristic_importance(trimmed),
        metadata: serde_json::json!({
            "new_memory_type": "semantic",
            "tags": ["learning", "session_end"],
            "task": task.unwrap_or_default(),
            "success": success,
            "project_id": project_id,
            "agent_type": agent_id,
        }),
    })
}

pub fn store_session_learning(
    session_id: &str,
    agent_id: &str,
    content: &str,
    task: Option<&str>,
    success: bool,
) -> Result<Option<String>> {
    let Some(record) = build_learning_record(session_id, agent_id, content, task, success) else {
        return Ok(None);
    };

    match resolve_memory_backend_preference() {
        Some(choice) => store_learning_with_backend(choice, &record),
        None => store_learning_with_backend(BackendChoice::Kuzu, &record)
            .or_else(|_| store_learning_with_backend(BackendChoice::Sqlite, &record)),
    }
}

fn store_learning_with_backend(
    choice: BackendChoice,
    record: &SessionLearningRecord,
) -> Result<Option<String>> {
    self::backend::open_runtime_backend(choice)?.store_session_learning(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::home_env_lock;
    use rusqlite::params;

    // -----------------------------------------------------------------------
    // SQLite tests (existing)
    // -----------------------------------------------------------------------

    #[test]
    fn sqlite_session_listing_reads_schema() -> Result<()> {
        let conn = SqliteConnection::open_in_memory()?;
        conn.execute_batch(SQLITE_SCHEMA)?;
        conn.execute(
            "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
            params!["test_sess", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
        )?;
        conn.execute(
            "INSERT INTO session_agents (session_id, agent_id, first_used, last_used) VALUES (?1, ?2, ?3, ?4)",
            params!["test_sess", "agent1", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
        )?;
        conn.execute(
            "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{}', ?7, ?8)",
            params!["m1", "test_sess", "agent1", "conversation", "Hello", "world", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
        )?;
        let sessions = list_sqlite_sessions_from_conn(&conn)?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].memory_count, 1);
        Ok(())
    }

    #[test]
    fn retrieve_prompt_context_memories_reads_sqlite_backend() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let prev_home = std::env::var_os("HOME");
        let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("AMPLIHACK_MEMORY_BACKEND", "sqlite");
        }

        let conn = open_sqlite_memory_db()?;
        conn.execute(
            "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
            params!["prompt-session", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
        )?;
        conn.execute(
            "INSERT INTO session_agents (session_id, agent_id, first_used, last_used) VALUES (?1, ?2, ?3, ?4)",
            params![
                "prompt-session",
                "agent1",
                "2026-01-02T03:04:05",
                "2026-01-02T03:04:05"
            ],
        )?;
        conn.execute(
            "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                "m1",
                "prompt-session",
                "agent1",
                "learning",
                "Fix CI",
                "To fix CI, rerun cargo fmt and cargo clippy before pushing.",
                r#"{"new_memory_type":"semantic"}"#,
                8,
                "2026-01-02T03:04:05",
                "2099-01-02T03:04:05"
            ],
        )?;
        conn.execute(
            "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                "m2",
                "prompt-session",
                "agent1",
                "context",
                "Temporary note",
                "This is only temporary working memory.",
                r#"{"new_memory_type":"working"}"#,
                10,
                "2026-01-02T03:04:05",
                "2099-01-02T03:04:05"
            ],
        )?;

        let memories = retrieve_prompt_context_memories("prompt-session", "fix ci", 2000)?;

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match prev_backend {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
        }

        assert_eq!(memories.len(), 1);
        assert!(memories[0].content.contains("rerun cargo fmt"));
        assert_eq!(memories[0].code_context, None);
        Ok(())
    }

    #[test]
    fn retrieve_prompt_context_memories_enriches_kuzu_code_context() -> Result<()> {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join(".amplihack").join("kuzu_db");
        let prev_home = std::env::var_os("HOME");
        let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("AMPLIHACK_MEMORY_BACKEND", "kuzu");
            std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", &db_path);
            std::env::set_var("AMPLIHACK_KUZU_DB_PATH", &db_path);
        }

        let record = SessionLearningRecord {
            session_id: "prompt-session".to_string(),
            agent_id: "agent1".to_string(),
            content: "Investigated helper behavior in src/example/module.py.".to_string(),
            title: "Helper behavior".to_string(),
            metadata: serde_json::json!({
                "new_memory_type": "semantic",
                "file": "src/example/module.py"
            }),
            importance: 8,
        };
        let memory_id = store_learning_kuzu(&record)?.expect("memory should be stored");

        let json_path = dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::json!({
                "files": [
                    {"path":"src/example/module.py","language":"python","lines_of_code":10},
                    {"path":"src/example/utils.py","language":"python","lines_of_code":5}
                ],
                "classes": [
                    {"id":"class:Example","name":"Example","file_path":"src/example/module.py","line_number":1}
                ],
                "functions": [
                    {"id":"func:Example.process","name":"process","file_path":"src/example/module.py","line_number":2,"class_id":"class:Example"},
                    {"id":"func:helper","name":"helper","file_path":"src/example/utils.py","line_number":1,"signature":"def helper()","docstring":"Helper function"}
                ],
                "imports": [],
                "relationships": [
                    {"type":"CALLS","source_id":"func:Example.process","target_id":"func:helper"}
                ]
            })
            .to_string(),
        )?;
        super::code_graph::import_blarify_json(&json_path, Some(&db_path))?;

        let memories = retrieve_prompt_context_memories("prompt-session", "helper", 2000)?;

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match prev_backend {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
        }
        match prev_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(memories.len(), 1);
        assert!(memories[0].content.contains("Investigated helper behavior"));
        let code_context = memories[0]
            .code_context
            .as_deref()
            .expect("kuzu-backed prompt memory should include code context");
        assert!(code_context.contains("**Related Files:**"));
        assert!(code_context.contains("src/example/module.py"));
        assert!(code_context.contains("**Related Functions:**"));
        assert!(code_context.contains("helper"));
        assert!(!memory_id.is_empty());
        Ok(())
    }

    #[test]
    fn resolve_kuzu_memory_db_path_prefers_env_override() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let override_path = dir.path().join("project-kuzu");
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", &override_path) };

        let resolved = resolve_kuzu_memory_db_path()?;

        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(resolved, override_path);
        Ok(())
    }

    #[test]
    fn resolve_kuzu_memory_db_path_prefers_backend_neutral_override() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let override_path = dir.path().join("project-graph");
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", &override_path) };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", dir.path().join("project-kuzu")) };

        let resolved = resolve_kuzu_memory_db_path()?;

        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(resolved, override_path);
        Ok(())
    }

    #[test]
    fn select_prompt_context_memories_respects_token_budget() {
        let memories = vec![
            MemoryRecord {
                memory_id: "m-large".to_string(),
                memory_type: "learning".to_string(),
                title: "Large".to_string(),
                content: "x".repeat(200),
                metadata: serde_json::json!({"new_memory_type": "semantic"}),
                importance: Some(10),
                accessed_at: Some("2099-01-02T03:04:05".to_string()),
                expires_at: None,
            },
            MemoryRecord {
                memory_id: "m-small".to_string(),
                memory_type: "learning".to_string(),
                title: "Small".to_string(),
                content: "fix ci quickly".to_string(),
                metadata: serde_json::json!({"new_memory_type": "semantic"}),
                importance: Some(1),
                accessed_at: Some("2099-01-02T03:04:05".to_string()),
                expires_at: None,
            },
        ];

        let selected = select_prompt_context_memories(memories, "fix ci", 10);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].memory_id, "m-small");
        assert_eq!(selected[0].content, "fix ci quickly");
    }

    #[test]
    fn build_learning_record_uses_semantic_metadata() {
        let record = build_learning_record(
            "sess-1",
            "analyzer",
            "Fixed CI by running cargo fmt and clippy locally before push.",
            Some("stabilize CI"),
            true,
        )
        .expect("record should be created");

        assert!(record.content.starts_with("Agent analyzer:"));
        assert_eq!(
            record
                .metadata
                .get("new_memory_type")
                .and_then(JsonValue::as_str),
            Some("semantic")
        );
        assert_eq!(
            record.metadata.get("task").and_then(JsonValue::as_str),
            Some("stabilize CI")
        );
    }

    // -----------------------------------------------------------------------
    // BackendChoice / TransferFormat unit tests
    // -----------------------------------------------------------------------

    /// BackendChoice::parse must accept "kuzu" and "sqlite" and reject anything else.
    ///
    /// These tests are purely logic-level and do not touch the kuzu C++ FFI.
    /// They document the expected API contract for callers of the memory backend.
    #[test]
    fn backend_choice_parse_kuzu() {
        assert_eq!(BackendChoice::parse("kuzu").unwrap(), BackendChoice::Kuzu);
    }

    #[test]
    fn backend_choice_parse_sqlite() {
        assert_eq!(
            BackendChoice::parse("sqlite").unwrap(),
            BackendChoice::Sqlite
        );
    }

    #[test]
    fn backend_choice_parse_invalid_returns_error() {
        assert!(
            BackendChoice::parse("postgres").is_err(),
            "Unknown backend names must be rejected"
        );
        assert!(
            BackendChoice::parse("").is_err(),
            "Empty string must be rejected"
        );
        assert!(
            BackendChoice::parse("KUZU").is_err(),
            "Case-sensitive: 'KUZU' is not 'kuzu'"
        );
    }

    #[test]
    fn transfer_format_parse_json() {
        assert_eq!(TransferFormat::parse("json").unwrap(), TransferFormat::Json);
    }

    #[test]
    fn transfer_format_parse_kuzu() {
        assert_eq!(TransferFormat::parse("kuzu").unwrap(), TransferFormat::Kuzu);
    }

    #[test]
    fn transfer_format_parse_invalid_returns_error() {
        assert!(
            TransferFormat::parse("csv").is_err(),
            "Unsupported formats must be rejected"
        );
        assert!(
            TransferFormat::parse("").is_err(),
            "Empty string must be rejected"
        );
    }

    // -----------------------------------------------------------------------
    // KuzuValue conversion unit tests
    // -----------------------------------------------------------------------

    /// kuzu_value_to_string must convert all scalar value variants to strings.
    /// These tests exercise the Rust-side value marshaling layer.
    #[test]
    fn kuzu_value_to_string_handles_string_variant() {
        let val = KuzuValue::String("hello".to_string());
        assert_eq!(kuzu_value_to_string(&val), "hello");
    }

    #[test]
    fn kuzu_value_to_string_handles_null() {
        let val = KuzuValue::Null(kuzu::LogicalType::String);
        assert_eq!(
            kuzu_value_to_string(&val),
            "",
            "Null must convert to empty string"
        );
    }

    #[test]
    fn kuzu_value_to_string_handles_non_string_via_display() {
        let val = KuzuValue::Int64(42);
        let s = kuzu_value_to_string(&val);
        assert!(
            s.contains("42"),
            "Int64(42) should display as a string containing '42', got: {s}"
        );
    }

    /// kuzu_value_to_i64 must extract integer values from all numeric variants.
    #[test]
    fn kuzu_value_to_i64_extracts_int64() {
        assert_eq!(kuzu_value_to_i64(&KuzuValue::Int64(99)), Some(99));
    }

    #[test]
    fn kuzu_value_to_i64_extracts_int32() {
        assert_eq!(kuzu_value_to_i64(&KuzuValue::Int32(7)), Some(7));
    }

    #[test]
    fn kuzu_value_to_i64_extracts_uint32() {
        assert_eq!(kuzu_value_to_i64(&KuzuValue::UInt32(5)), Some(5));
    }

    #[test]
    fn kuzu_value_to_i64_returns_none_for_non_numeric() {
        let val = KuzuValue::String("abc".to_string());
        assert_eq!(
            kuzu_value_to_i64(&val),
            None,
            "Non-numeric value must return None"
        );
    }

    #[test]
    fn kuzu_value_to_i64_extracts_double_as_truncated_i64() {
        assert_eq!(kuzu_value_to_i64(&KuzuValue::Double(3.9)), Some(3));
    }

    // -----------------------------------------------------------------------
    // parse_json_value unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_json_value_empty_string_returns_empty_object() {
        let val = parse_json_value("").unwrap();
        assert!(
            val.is_object(),
            "Empty string must parse to empty JSON object"
        );
        assert!(val.as_object().unwrap().is_empty());
    }

    #[test]
    fn parse_json_value_valid_json_parses_correctly() {
        let val = parse_json_value(r#"{"key": "value"}"#).unwrap();
        assert_eq!(val["key"], "value");
    }

    #[test]
    fn parse_json_value_invalid_json_returns_error() {
        assert!(
            parse_json_value("{not valid json}").is_err(),
            "Invalid JSON must return an error"
        );
    }
}
