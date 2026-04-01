use super::super::super::*;
use super::super::super::learning::build_memory_id;
use super::values::{graph_i64, graph_rows};
use super::{GraphDbConnection, GraphDbValue};
use anyhow::Result;
use chrono::{TimeZone as _, Utc};
use time::OffsetDateTime;

pub(super) fn store_learning_graph_with_conn(
    conn: &GraphDbConnection<'_>,
    record: &SessionLearningRecord,
) -> Result<Option<String>> {
    let duplicate_rows = graph_rows(
        conn,
        "MATCH (s:Session {session_id: $session_id})-[:CONTRIBUTES_TO_SEMANTIC]->(m:SemanticMemory) WHERE m.agent_id = $agent_id AND m.content = $content RETURN COUNT(m)",
        vec![
            (
                "session_id",
                GraphDbValue::String(record.session_id.clone()),
            ),
            ("agent_id", GraphDbValue::String(record.agent_id.clone())),
            ("content", GraphDbValue::String(record.content.clone())),
        ],
    )?;
    let duplicate_count = duplicate_rows
        .first()
        .map(|row| graph_i64(row.first()).unwrap_or(0))
        .unwrap_or(0);
    if duplicate_count > 0 {
        return Ok(None);
    }

    // Single clock read: derive the RFC3339 string from the same OffsetDateTime used for
    // Kuzu TIMESTAMP parameters rather than taking a second snapshot with Utc::now().
    let now = OffsetDateTime::now_utc();
    let now_str = chrono::Utc
        .timestamp_opt(now.unix_timestamp(), now.nanosecond())
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let memory_id = build_memory_id(record, &now_str);
    let metadata = serde_json::to_string(&record.metadata)?;
    let tags = serde_json::to_string(&["learning", "session_end"])?;

    ensure_graph_session(conn, &record.session_id, now)?;
    ensure_graph_agent(conn, &record.agent_id, now)?;

    let mut create_memory = conn.prepare(
        "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, category: $category, confidence_score: $confidence_score, last_updated: $last_updated, version: $version, title: $title, metadata: $metadata, tags: $tags, created_at: $created_at, accessed_at: $accessed_at, agent_id: $agent_id})",
    )?;
    conn.execute(
        &mut create_memory,
        vec![
            ("memory_id", GraphDbValue::String(memory_id.clone())),
            ("concept", GraphDbValue::String(record.title.clone())),
            ("content", GraphDbValue::String(record.content.clone())),
            ("category", GraphDbValue::String("session_end".to_string())),
            ("confidence_score", GraphDbValue::Double(1.0)),
            ("last_updated", GraphDbValue::Timestamp(now)),
            ("version", GraphDbValue::Int64(1)),
            ("title", GraphDbValue::String(record.title.clone())),
            ("metadata", GraphDbValue::String(metadata)),
            ("tags", GraphDbValue::String(tags)),
            ("created_at", GraphDbValue::Timestamp(now)),
            ("accessed_at", GraphDbValue::Timestamp(now)),
            ("agent_id", GraphDbValue::String(record.agent_id.clone())),
        ],
    )?;

    let mut create_link = conn.prepare(
        "MATCH (s:Session {session_id: $session_id}), (m:SemanticMemory {memory_id: $memory_id}) CREATE (s)-[:CONTRIBUTES_TO_SEMANTIC {contribution_type: $contribution_type, timestamp: $timestamp, delta: $delta}]->(m)",
    )?;
    conn.execute(
        &mut create_link,
        vec![
            (
                "session_id",
                GraphDbValue::String(record.session_id.clone()),
            ),
            ("memory_id", GraphDbValue::String(memory_id.clone())),
            (
                "contribution_type",
                GraphDbValue::String("created".to_string()),
            ),
            ("timestamp", GraphDbValue::Timestamp(now)),
            (
                "delta",
                GraphDbValue::String("initial_creation".to_string()),
            ),
        ],
    )?;

    Ok(Some(memory_id))
}

fn ensure_graph_session(
    conn: &GraphDbConnection<'_>,
    session_id: &str,
    now: OffsetDateTime,
) -> Result<()> {
    let count_rows = graph_rows(
        conn,
        "MATCH (s:Session {session_id: $session_id}) RETURN COUNT(s)",
        vec![("session_id", GraphDbValue::String(session_id.to_string()))],
    )?;
    let count = count_rows
        .first()
        .map(|row| graph_i64(row.first()).unwrap_or(0))
        .unwrap_or(0);

    if count == 0 {
        let mut create = conn.prepare(
            "CREATE (s:Session {session_id: $session_id, start_time: $start_time, end_time: NULL, user_id: '', context: '', status: $status, created_at: $created_at, last_accessed: $last_accessed, metadata: $metadata})",
        )?;
        conn.execute(
            &mut create,
            vec![
                ("session_id", GraphDbValue::String(session_id.to_string())),
                ("start_time", GraphDbValue::Timestamp(now)),
                ("status", GraphDbValue::String("active".to_string())),
                ("created_at", GraphDbValue::Timestamp(now)),
                ("last_accessed", GraphDbValue::Timestamp(now)),
                ("metadata", GraphDbValue::String("{}".to_string())),
            ],
        )?;
    } else {
        let mut update = conn.prepare(
            "MATCH (s:Session {session_id: $session_id}) SET s.last_accessed = $last_accessed",
        )?;
        conn.execute(
            &mut update,
            vec![
                ("session_id", GraphDbValue::String(session_id.to_string())),
                ("last_accessed", GraphDbValue::Timestamp(now)),
            ],
        )?;
    }

    Ok(())
}

fn ensure_graph_agent(
    conn: &GraphDbConnection<'_>,
    agent_id: &str,
    now: OffsetDateTime,
) -> Result<()> {
    let count_rows = graph_rows(
        conn,
        "MATCH (a:Agent {agent_id: $agent_id}) RETURN COUNT(a)",
        vec![("agent_id", GraphDbValue::String(agent_id.to_string()))],
    )?;
    let count = count_rows
        .first()
        .map(|row| graph_i64(row.first()).unwrap_or(0))
        .unwrap_or(0);

    if count == 0 {
        let mut create = conn.prepare(
            "CREATE (a:Agent {agent_id: $agent_id, name: $name, first_used: $first_used, last_used: $last_used})",
        )?;
        conn.execute(
            &mut create,
            vec![
                ("agent_id", GraphDbValue::String(agent_id.to_string())),
                ("name", GraphDbValue::String(agent_id.to_string())),
                ("first_used", GraphDbValue::Timestamp(now)),
                ("last_used", GraphDbValue::Timestamp(now)),
            ],
        )?;
    } else {
        let mut update =
            conn.prepare("MATCH (a:Agent {agent_id: $agent_id}) SET a.last_used = $last_used")?;
        conn.execute(
            &mut update,
            vec![
                ("agent_id", GraphDbValue::String(agent_id.to_string())),
                ("last_used", GraphDbValue::Timestamp(now)),
            ],
        )?;
    }

    Ok(())
}
