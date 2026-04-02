use crate::commands::memory::HIERARCHICAL_SCHEMA;
use crate::commands::memory::backend::graph_db::{
    GraphDbConnection, GraphDbHandle, GraphDbValue, graph_rows, graph_string,
};
use anyhow::Result;
use std::path::Path;

pub(super) fn with_hierarchical_graph_conn<T>(
    db_path: &Path,
    f: impl FnOnce(&GraphDbConnection<'_>) -> Result<T>,
) -> Result<T> {
    GraphDbHandle::open_at_path(db_path)?.with_initialized_conn(init_hierarchical_schema, f)
}

pub(super) fn init_hierarchical_schema(conn: &GraphDbConnection<'_>) -> Result<()> {
    for statement in HIERARCHICAL_SCHEMA {
        conn.query(statement)?;
    }
    Ok(())
}

pub(super) fn clear_hierarchical_agent_data(
    conn: &GraphDbConnection<'_>,
    agent_name: &str,
) -> Result<()> {
    for query in [
        "MATCH (a:SemanticMemory {agent_id: $aid})-[r:SIMILAR_TO]->() DELETE r",
        "MATCH ()-[r:SIMILAR_TO]->(b:SemanticMemory {agent_id: $aid}) DELETE r",
        "MATCH (s:SemanticMemory {agent_id: $aid})-[r:DERIVES_FROM]->() DELETE r",
        "MATCH (n:SemanticMemory {agent_id: $aid})-[r:SUPERSEDES]->() DELETE r",
        "MATCH ()-[r:SUPERSEDES]->(o:SemanticMemory {agent_id: $aid}) DELETE r",
        "MATCH (n:SemanticMemory {agent_id: $aid})-[r:TRANSITIONED_TO]->() DELETE r",
        "MATCH ()-[r:TRANSITIONED_TO]->(o:SemanticMemory {agent_id: $aid}) DELETE r",
        "MATCH (m:SemanticMemory {agent_id: $aid}) DELETE m",
        "MATCH (e:EpisodicMemory {agent_id: $aid}) DELETE e",
    ] {
        graph_rows(
            conn,
            query,
            vec![("aid", GraphDbValue::String(agent_name.to_string()))],
        )?;
    }
    Ok(())
}

pub(super) fn get_existing_hierarchical_ids(
    conn: &GraphDbConnection<'_>,
    agent_name: &str,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for query in [
        "MATCH (m:SemanticMemory {agent_id: $aid}) RETURN m.memory_id",
        "MATCH (e:EpisodicMemory {agent_id: $aid}) RETURN e.memory_id",
    ] {
        let rows = graph_rows(
            conn,
            query,
            vec![("aid", GraphDbValue::String(agent_name.to_string()))],
        )?;
        for row in rows {
            ids.push(graph_string(row.first())?);
        }
    }
    Ok(ids)
}

pub(super) fn create_hierarchical_edge(
    conn: &GraphDbConnection<'_>,
    query: &str,
    params: Vec<(&str, GraphDbValue)>,
) -> Result<bool> {
    let mut prepared = conn.prepare(query)?;
    Ok(conn.execute(&mut prepared, params).is_ok())
}
