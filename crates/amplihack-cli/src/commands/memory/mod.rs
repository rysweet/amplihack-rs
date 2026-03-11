//! Native memory commands (`tree`, `export`, `import`, `clean`).

pub mod clean;
pub mod transfer;
pub mod tree;

pub use clean::run_clean;
pub use transfer::{run_export, run_import};
pub use tree::run_tree;

use anyhow::{Context, Result};
use kuzu::{
    Connection as KuzuConnection, Database as KuzuDatabase, SystemConfig, Value as KuzuValue,
};
use rusqlite::{Connection as SqliteConnection, params};
use serde_json::Value as JsonValue;
use std::fs;
use std::path::PathBuf;

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
pub(crate) struct SessionSummary {
    pub(crate) session_id: String,
    pub(crate) memory_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryRecord {
    pub(crate) memory_type: String,
    pub(crate) title: String,
    pub(crate) metadata: JsonValue,
    pub(crate) importance: Option<i64>,
    pub(crate) expires_at: Option<String>,
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

pub(crate) fn list_sqlite_sessions() -> Result<Vec<SessionSummary>> {
    let conn = open_sqlite_memory_db()?;
    list_sqlite_sessions_from_conn(&conn)
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
        "SELECT memory_type, title, metadata, importance, expires_at FROM memory_entries WHERE session_id = ?1 AND (expires_at IS NULL OR expires_at > datetime('now'))",
    );
    if memory_type.is_some() {
        sql.push_str(" AND memory_type = ?2");
    }
    sql.push_str(" ORDER BY accessed_at DESC, importance DESC");
    let mut stmt = conn.prepare(&sql)?;
    let mapper = |row: &rusqlite::Row<'_>| -> rusqlite::Result<MemoryRecord> {
        let metadata_raw: Option<String> = row.get(2)?;
        Ok(MemoryRecord {
            memory_type: row.get(0)?,
            title: row.get(1)?,
            metadata: metadata_raw
                .as_deref()
                .map(parse_json_value)
                .transpose()
                .map_err(to_sqlite_err)?
                .unwrap_or(JsonValue::Object(Default::default())),
            importance: row.get(3)?,
            expires_at: row.get(4)?,
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
    let path = home_dir()?.join(".amplihack").join("memory_kuzu.db");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(KuzuDatabase::new(path, SystemConfig::default())?)
}

pub(crate) fn init_kuzu_backend_schema(conn: &KuzuConnection<'_>) -> Result<()> {
    for statement in KUZU_BACKEND_SCHEMA {
        conn.query(statement)?;
    }
    Ok(())
}

pub(crate) fn list_kuzu_sessions() -> Result<Vec<SessionSummary>> {
    let db = open_kuzu_memory_db()?;
    let conn = KuzuConnection::new(&db)?;
    init_kuzu_backend_schema(&conn)?;
    list_kuzu_sessions_from_conn(&conn)
}

pub(crate) fn list_kuzu_sessions_from_conn(
    conn: &KuzuConnection<'_>,
) -> Result<Vec<SessionSummary>> {
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
    let rows = kuzu_rows(
        conn,
        "MATCH (a:Agent) RETURN a.agent_id ORDER BY a.agent_id ASC",
        vec![],
    )?;
    let mut counts = Vec::new();
    for row in rows {
        let agent_id = kuzu_string(row.first())?;
        let mut total = 0usize;
        for (label, _) in KUZU_MEMORY_TABLES {
            let query = format!("MATCH (m:{label} {{agent_id: $agent_id}}) RETURN COUNT(m)");
            let count_rows = kuzu_rows(
                conn,
                &query,
                vec![("agent_id", KuzuValue::String(agent_id.clone()))],
            )?;
            if let Some(first_row) = count_rows.first() {
                total += kuzu_i64(first_row.first())? as usize;
            }
        }
        if total > 0 {
            counts.push((agent_id, total));
        }
    }
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

pub(crate) fn kuzu_rows(
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
        memory_type: label
            .strip_suffix("Memory")
            .unwrap_or(label)
            .to_ascii_lowercase(),
        title: property_string(props, "title")
            .or_else(|| property_string(props, "concept"))
            .or_else(|| property_string(props, "procedure_name"))
            .unwrap_or_default(),
        metadata,
        importance,
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

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
}
