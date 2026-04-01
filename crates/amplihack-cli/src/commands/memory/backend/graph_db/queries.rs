use super::super::super::*;
use super::schema::GRAPH_MEMORY_TABLES;
use super::values::{
    graph_i64, graph_rows, graph_string, graph_value_to_string, memory_from_graph_node,
};
use super::{GraphDbConnection, GraphDbValue};
use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;

pub fn list_graph_sessions_from_conn(conn: &GraphDbConnection<'_>) -> Result<Vec<SessionSummary>> {
    let rows = graph_rows(
        conn,
        "MATCH (s:Session) RETURN s.session_id, s.created_at, s.last_accessed, s.metadata ORDER BY s.last_accessed DESC",
        vec![],
    )?;
    let mut sessions = Vec::new();
    for row in rows {
        let session_id = graph_string(row.first())?;
        let memories = query_graph_memories_for_session(conn, &session_id, None)?;
        sessions.push(SessionSummary {
            session_id,
            memory_count: memories.len(),
        });
    }
    Ok(sessions)
}

pub(crate) fn query_graph_memories_for_session(
    conn: &GraphDbConnection<'_>,
    session_id: &str,
    memory_type: Option<&str>,
) -> Result<Vec<MemoryRecord>> {
    let now = Utc::now();
    let mut memories = Vec::new();
    for (label, rel_name, _has_expires_at) in GRAPH_MEMORY_TABLES {
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
        let rows = graph_rows(
            conn,
            &query,
            vec![("session_id", GraphDbValue::String(session_id.to_string()))],
        )?;
        for row in rows {
            if let Some(value) = row.first() {
                let record = memory_from_graph_node(value, session_id, label)?;
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

pub(crate) fn collect_graph_db_agent_counts(
    conn: &GraphDbConnection<'_>,
) -> Result<Vec<(String, usize)>> {
    let now = Utc::now();
    let mut totals: HashMap<String, usize> = HashMap::new();
    for (label, _, has_expires_at) in GRAPH_MEMORY_TABLES {
        if *has_expires_at {
            // For tables that carry an `expires_at` timestamp, fetch individual
            // records and apply Rust-side expiry filtering before counting.
            // This mirrors the SQLite `WHERE expires_at IS NULL OR expires_at >
            // datetime('now')` clause without relying on a Kùzu Cypher
            // timestamp function whose exact name is unstable.
            let rows = graph_rows(
                conn,
                &format!("MATCH (m:{label}) RETURN m.agent_id, m.expires_at"),
                vec![],
            )?;
            for row in rows {
                let agent_id = graph_string(row.first())?;
                if agent_id.is_empty() {
                    continue;
                }
                let expires_at_str = row.get(1).map(graph_value_to_string).unwrap_or_default();
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
            let rows = graph_rows(
                conn,
                &format!("MATCH (m:{label}) RETURN m.agent_id, COUNT(m)"),
                vec![],
            )?;
            for row in rows {
                let agent_id = graph_string(row.first())?;
                let count = graph_i64(row.get(1))? as usize;
                *totals.entry(agent_id).or_insert(0) += count;
            }
        }
    }

    let mut counts: Vec<(String, usize)> =
        totals.into_iter().filter(|(_, total)| *total > 0).collect();
    counts.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(counts)
}

pub(super) fn delete_graph_session_with_conn(
    conn: &GraphDbConnection<'_>,
    session_id: &str,
) -> Result<bool> {
    let exists = graph_rows(
        conn,
        "MATCH (s:Session {session_id: $session_id}) RETURN COUNT(s)",
        vec![("session_id", GraphDbValue::String(session_id.to_string()))],
    )?;
    let existing = exists
        .first()
        .map(|row| graph_i64(row.first()).unwrap_or(0))
        .unwrap_or(0);
    if existing == 0 {
        return Ok(false);
    }
    for (label, rel_name, _) in GRAPH_MEMORY_TABLES {
        let query = format!(
            "MATCH (s:Session {{session_id: $session_id}})-[:{rel_name}]->(m:{label}) DETACH DELETE m"
        );
        graph_rows(
            conn,
            &query,
            vec![("session_id", GraphDbValue::String(session_id.to_string()))],
        )?;
    }
    graph_rows(
        conn,
        "MATCH (s:Session {session_id: $session_id}) DETACH DELETE s",
        vec![("session_id", GraphDbValue::String(session_id.to_string()))],
    )?;
    Ok(true)
}
