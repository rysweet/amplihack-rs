use super::super::*;
use super::{MemoryRuntimeBackend, MemorySessionBackend, MemoryTreeBackend};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use kuzu::SystemConfig;
pub(crate) use kuzu::{Connection as KuzuConnection, Database as KuzuDatabase, Value as KuzuValue};
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use time::OffsetDateTime;

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

/// `(node_label, relationship_name, has_expires_at)`.
///
/// `has_expires_at` is true when the node table includes an `expires_at`
/// TIMESTAMP column in the schema.  SemanticMemory and ProceduralMemory
/// represent long-lived facts with no TTL, so they never expire.
pub(crate) const KUZU_MEMORY_TABLES: &[(&str, &str, bool)] = &[
    ("EpisodicMemory", "CONTAINS_EPISODIC", true),
    ("SemanticMemory", "CONTRIBUTES_TO_SEMANTIC", false),
    ("ProceduralMemory", "USES_PROCEDURE", false),
    ("ProspectiveMemory", "CREATES_INTENTION", true),
    ("WorkingMemory", "CONTAINS_WORKING", true),
];

pub(crate) struct KuzuBackend {
    db: KuzuDatabase,
}

impl KuzuBackend {
    pub(crate) fn open() -> Result<Self> {
        let db = open_kuzu_memory_db()?;
        let backend = Self { db };
        backend.with_conn(init_kuzu_backend_schema)?;
        Ok(backend)
    }

    fn with_conn<T>(&self, f: impl FnOnce(&KuzuConnection<'_>) -> Result<T>) -> Result<T> {
        let conn = KuzuConnection::new(&self.db).context("failed to connect to Kùzu memory DB")?;
        f(&conn)
    }
}

impl MemoryTreeBackend for KuzuBackend {
    fn backend_name(&self) -> &'static str {
        KUZU_TREE_BACKEND_NAME
    }

    fn load_session_rows(
        &self,
        session_id: Option<&str>,
        memory_type: Option<&str>,
    ) -> Result<Vec<(SessionSummary, Vec<MemoryRecord>)>> {
        self.with_conn(|conn| {
            let mut sessions = list_kuzu_sessions_from_conn(conn)?;
            if let Some(session_id) = session_id {
                sessions.retain(|session| session.session_id == session_id);
            }

            let mut session_rows = Vec::new();
            for session in sessions {
                let memories =
                    query_kuzu_memories_for_session(conn, &session.session_id, memory_type)?;
                let memory_count = memories.len();
                let mut session = session;
                session.memory_count = memory_count;
                session_rows.push((session, memories));
            }
            Ok(session_rows)
        })
    }

    fn collect_agent_counts(&self) -> Result<Vec<(String, usize)>> {
        self.with_conn(collect_kuzu_agent_counts)
    }
}

impl MemorySessionBackend for KuzuBackend {
    fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.with_conn(list_kuzu_sessions_from_conn)
    }

    fn delete_session(&self, session_id: &str) -> Result<bool> {
        self.with_conn(|conn| delete_kuzu_session_with_conn(conn, session_id))
    }
}

impl MemoryRuntimeBackend for KuzuBackend {
    fn load_prompt_context_memories(&self, session_id: &str) -> Result<Vec<MemoryRecord>> {
        self.with_conn(|conn| query_kuzu_memories_for_session(conn, session_id, None))
    }

    fn store_session_learning(&self, record: &SessionLearningRecord) -> Result<Option<String>> {
        self.with_conn(|conn| store_learning_kuzu_with_conn(conn, record))
    }
}

pub(crate) fn open_kuzu_memory_db() -> Result<KuzuDatabase> {
    let path = resolve_memory_graph_db_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(KuzuDatabase::new(path, SystemConfig::default())?)
}

pub(crate) fn resolve_memory_graph_db_path() -> Result<PathBuf> {
    fn validate_graph_db_override(path: PathBuf, env_var: &str) -> Option<PathBuf> {
        if !path.is_absolute() {
            tracing::warn!(
                env_var,
                path = %path.display(),
                "ignoring non-absolute memory graph DB override"
            );
            return None;
        }
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            tracing::warn!(
                env_var,
                path = %path.display(),
                "ignoring memory graph DB override with parent traversal"
            );
            return None;
        }
        for blocked in [Path::new("/proc"), Path::new("/sys"), Path::new("/dev")] {
            if path.starts_with(blocked) {
                tracing::warn!(
                    env_var,
                    path = %path.display(),
                    blocked = %blocked.display(),
                    "ignoring memory graph DB override with unsafe path prefix"
                );
                return None;
            }
        }
        Some(path)
    }

    if let Some(path) = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH")
        && !path.is_empty()
        && let Some(path) =
            validate_graph_db_override(PathBuf::from(path), "AMPLIHACK_GRAPH_DB_PATH")
    {
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("AMPLIHACK_KUZU_DB_PATH")
        && !path.is_empty()
        && let Some(path) =
            validate_graph_db_override(PathBuf::from(path), "AMPLIHACK_KUZU_DB_PATH")
    {
        return Ok(path);
    }

    let home = home_dir()?;
    let neutral = home.join(".amplihack").join("memory_graph.db");
    let legacy = home.join(".amplihack").join("memory_kuzu.db");
    if legacy.exists() && !neutral.exists() {
        return Ok(legacy);
    }
    Ok(neutral)
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
        let memories = query_kuzu_memories_for_session(conn, &session_id, None)?;
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
    memory_type: Option<&str>,
) -> Result<Vec<MemoryRecord>> {
    let now = Utc::now();
    let mut memories = Vec::new();
    for (label, rel_name, _has_expires_at) in KUZU_MEMORY_TABLES {
        // When a memory_type filter is requested, skip tables whose normalised
        // type name (label stripped of "Memory" suffix, lowercased) does not
        // match.  This mirrors the SQLite `AND memory_type = ?2` clause.
        if let Some(requested_type) = memory_type {
            let label_type = label
                .strip_suffix("Memory")
                .unwrap_or(label)
                .to_ascii_lowercase();
            if label_type != requested_type {
                continue;
            }
        }

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
                let record = memory_from_kuzu_node(value, session_id, label)?;
                // Apply Rust-side expiry filter.  Records whose `expires_at`
                // timestamp is in the past are discarded; records with no
                // `expires_at` (or an unparseable value) are kept.
                if record
                    .expires_at
                    .as_deref()
                    .and_then(parse_memory_timestamp)
                    .is_some_and(|expiry| expiry <= now)
                {
                    continue;
                }
                memories.push(record);
            }
        }
    }
    Ok(memories)
}

pub(crate) fn collect_kuzu_agent_counts(conn: &KuzuConnection<'_>) -> Result<Vec<(String, usize)>> {
    let now = Utc::now();
    let mut totals: HashMap<String, usize> = HashMap::new();
    for (label, _, has_expires_at) in KUZU_MEMORY_TABLES {
        if *has_expires_at {
            // For tables that carry an `expires_at` timestamp, fetch individual
            // records and apply Rust-side expiry filtering before counting.
            // This mirrors the SQLite `WHERE expires_at IS NULL OR expires_at >
            // datetime('now')` clause without relying on a Kùzu Cypher
            // timestamp function whose exact name is unstable.
            let rows = kuzu_rows(
                conn,
                &format!("MATCH (m:{label}) RETURN m.agent_id, m.expires_at"),
                vec![],
            )?;
            for row in rows {
                let agent_id = kuzu_string(row.first())?;
                if agent_id.is_empty() {
                    continue;
                }
                let expires_at_str = row.get(1).map(kuzu_value_to_string).unwrap_or_default();
                let is_expired = if expires_at_str.is_empty() {
                    false
                } else {
                    parse_memory_timestamp(&expires_at_str).is_some_and(|t| t <= now)
                };
                if !is_expired {
                    *totals.entry(agent_id).or_insert(0) += 1;
                }
            }
        } else {
            // Tables without `expires_at` (SemanticMemory, ProceduralMemory)
            // hold long-lived facts that never expire; COUNT directly.
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
    }

    let mut counts: Vec<(String, usize)> =
        totals.into_iter().filter(|(_, total)| *total > 0).collect();
    counts.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(counts)
}

fn delete_kuzu_session_with_conn(conn: &KuzuConnection<'_>, session_id: &str) -> Result<bool> {
    let exists = kuzu_rows(
        conn,
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
    for (label, rel_name, _) in KUZU_MEMORY_TABLES {
        let query = format!(
            "MATCH (s:Session {{session_id: $session_id}})-[:{rel_name}]->(m:{label}) DETACH DELETE m"
        );
        kuzu_rows(
            conn,
            &query,
            vec![("session_id", KuzuValue::String(session_id.to_string()))],
        )?;
    }
    kuzu_rows(
        conn,
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
        KuzuValue::Timestamp(v) => {
            DateTime::<Utc>::from_timestamp(v.unix_timestamp(), v.nanosecond())
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| v.to_string())
        }
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

fn store_learning_kuzu_with_conn(
    conn: &KuzuConnection<'_>,
    record: &SessionLearningRecord,
) -> Result<Option<String>> {
    let duplicate_rows = kuzu_rows(
        conn,
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

    ensure_kuzu_session(conn, &record.session_id, now)?;
    ensure_kuzu_agent(conn, &record.agent_id, now)?;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
