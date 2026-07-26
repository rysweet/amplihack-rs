use super::super::super::backend::graph_db::{
    GraphDbConnection, GraphDbValue, graph_i64, graph_rows, graph_string,
};
use super::ensure_memory_code_link_schema;
use super::schema::{GRAPH_MEMORY_FILE_LINK_TABLES, GRAPH_MEMORY_FUNCTION_LINK_TABLES};
use anyhow::Result;
use time::OffsetDateTime;

fn normalize_match_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub(super) fn link_memories_to_code_files_in_conn(conn: &GraphDbConnection<'_>) -> Result<usize> {
    ensure_memory_code_link_schema(conn)?;

    let mut created = 0usize;
    let now = OffsetDateTime::now_utc();

    for (memory_type, rel_table) in GRAPH_MEMORY_FILE_LINK_TABLES {
        let memories = graph_rows(
            conn,
            &format!(
                "MATCH (m:{memory_type}) WHERE m.metadata IS NOT NULL RETURN m.memory_id, m.metadata"
            ),
            vec![],
        )?;

        for row in memories {
            let memory_id = graph_string(row.first())?;
            let metadata_raw = match row.get(1) {
                Some(value) => graph_string(Some(value))?,
                None => continue,
            };
            if metadata_raw.trim().is_empty() {
                continue;
            }

            let metadata = match serde_json::from_str::<serde_json::Value>(&metadata_raw) {
                Ok(metadata) => metadata,
                Err(error) => {
                    tracing::warn!(
                        %memory_id,
                        memory_type,
                        %error,
                        "invalid memory metadata JSON; skipping code-file linking"
                    );
                    continue;
                }
            };
            let Some(file_path) = metadata.get("file").and_then(|value| value.as_str()) else {
                continue;
            };
            let file_path = normalize_match_path(file_path);
            if file_path.trim().is_empty() {
                continue;
            }

            let matching_files = graph_rows(
                conn,
                "MATCH (cf:CodeFile) WHERE cf.file_path CONTAINS $file_path OR $file_path CONTAINS cf.file_path RETURN cf.file_id",
                vec![("file_path", GraphDbValue::String(file_path))],
            )?;

            for file_row in matching_files {
                let file_id = graph_string(file_row.first())?;
                let existing = graph_rows(
                    conn,
                    &format!(
                        "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[r:{rel_table}]->(cf:CodeFile {{file_id: $file_id}}) RETURN COUNT(r)"
                    ),
                    vec![
                        ("memory_id", GraphDbValue::String(memory_id.clone())),
                        ("file_id", GraphDbValue::String(file_id.clone())),
                    ],
                )?;
                let existing_count = existing
                    .first()
                    .map(|row| graph_i64(row.first()).unwrap_or(0))
                    .unwrap_or(0);
                if existing_count > 0 {
                    continue;
                }

                let mut create = conn.prepare(&format!(
                    "MATCH (m:{memory_type} {{memory_id: $memory_id}}) MATCH (cf:CodeFile {{file_id: $file_id}}) CREATE (m)-[:{rel_table} {{relevance_score: $relevance_score, context: $context, timestamp: $timestamp}}]->(cf)"
                ))?;
                conn.execute(
                    &mut create,
                    vec![
                        ("memory_id", GraphDbValue::String(memory_id.clone())),
                        ("file_id", GraphDbValue::String(file_id)),
                        ("relevance_score", GraphDbValue::Double(1.0)),
                        (
                            "context",
                            GraphDbValue::String("metadata_file_match".to_string()),
                        ),
                        ("timestamp", GraphDbValue::Timestamp(now)),
                    ],
                )?;
                created += 1;
            }
        }
    }

    for (memory_type, rel_table) in GRAPH_MEMORY_FUNCTION_LINK_TABLES {
        let memories = graph_rows(
            conn,
            &format!(
                "MATCH (m:{memory_type}) WHERE m.content IS NOT NULL RETURN m.memory_id, m.content"
            ),
            vec![],
        )?;

        for row in memories {
            let memory_id = graph_string(row.first())?;
            let content = match row.get(1) {
                Some(value) => graph_string(Some(value))?,
                None => continue,
            };
            if content.trim().is_empty() {
                continue;
            }

            let matching_functions = graph_rows(
                conn,
                "MATCH (f:CodeFunction) WHERE $content CONTAINS f.function_name RETURN f.function_id",
                vec![("content", GraphDbValue::String(content))],
            )?;

            for function_row in matching_functions {
                let function_id = graph_string(function_row.first())?;
                let existing = graph_rows(
                    conn,
                    &format!(
                        "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[r:{rel_table}]->(f:CodeFunction {{function_id: $function_id}}) RETURN COUNT(r)"
                    ),
                    vec![
                        ("memory_id", GraphDbValue::String(memory_id.clone())),
                        ("function_id", GraphDbValue::String(function_id.clone())),
                    ],
                )?;
                let existing_count = existing
                    .first()
                    .map(|row| graph_i64(row.first()).unwrap_or(0))
                    .unwrap_or(0);
                if existing_count > 0 {
                    continue;
                }

                let mut create = conn.prepare(&format!(
                    "MATCH (m:{memory_type} {{memory_id: $memory_id}}) MATCH (f:CodeFunction {{function_id: $function_id}}) CREATE (m)-[:{rel_table} {{relevance_score: $relevance_score, context: $context, timestamp: $timestamp}}]->(f)"
                ))?;
                conn.execute(
                    &mut create,
                    vec![
                        ("memory_id", GraphDbValue::String(memory_id.clone())),
                        ("function_id", GraphDbValue::String(function_id)),
                        ("relevance_score", GraphDbValue::Double(0.8)),
                        (
                            "context",
                            GraphDbValue::String("content_name_match".to_string()),
                        ),
                        ("timestamp", GraphDbValue::Timestamp(now)),
                    ],
                )?;
                created += 1;
            }
        }
    }

    Ok(created)
}
