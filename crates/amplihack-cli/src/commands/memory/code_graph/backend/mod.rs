mod import_ops;
mod memory_links;
mod reader;
mod relationships;
mod schema;
mod writer;

use super::super::backend::graph_db::{
    GraphDbConnection, GraphDbHandle, GraphDbValue, graph_i64, graph_rows,
    init_graph_backend_schema,
};
use super::*;
use chrono::DateTime;
use std::path::Path;
use time::OffsetDateTime;

pub(super) use reader::open_code_graph_reader;
pub(super) use writer::open_code_graph_writer;

pub(super) fn open_graph_db_code_graph_db(path_override: Option<&Path>) -> Result<GraphDbHandle> {
    let path = match path_override {
        Some(path) => path.to_path_buf(),
        None => default_code_graph_db_path()?,
    };
    let db = GraphDbHandle::open_at_path(&path)?;
    enforce_db_permissions(&path)?;
    Ok(db)
}

#[cfg(test)]
pub(crate) fn with_test_code_graph_conn<T>(
    path_override: Option<&Path>,
    f: impl FnOnce(&GraphDbConnection<'_>) -> Result<T>,
) -> Result<T> {
    let handle = open_graph_db_code_graph_db(path_override)?;
    handle.with_initialized_conn(ensure_memory_code_link_schema, f)
}

#[cfg(test)]
pub(crate) fn initialize_test_code_graph_db(path_override: Option<&Path>) -> Result<()> {
    with_test_code_graph_conn(path_override, |_| Ok(()))
}

fn init_graph_db_code_graph_schema(conn: &GraphDbConnection<'_>) -> Result<()> {
    for statement in schema::GRAPH_CODE_GRAPH_SCHEMA {
        conn.query(statement)?;
    }
    Ok(())
}

pub(super) fn ensure_memory_code_link_schema(conn: &GraphDbConnection<'_>) -> Result<()> {
    init_graph_backend_schema(conn)?;
    init_graph_db_code_graph_schema(conn)?;
    for (memory_type, rel_table) in schema::GRAPH_MEMORY_FILE_LINK_TABLES {
        conn.query(&format!(
            "CREATE REL TABLE IF NOT EXISTS {rel_table}(FROM {memory_type} TO CodeFile, relevance_score DOUBLE, context STRING, timestamp TIMESTAMP)"
        ))?;
    }
    for (memory_type, rel_table) in schema::GRAPH_MEMORY_FUNCTION_LINK_TABLES {
        conn.query(&format!(
            "CREATE REL TABLE IF NOT EXISTS {rel_table}(FROM {memory_type} TO CodeFunction, relevance_score DOUBLE, context STRING, timestamp TIMESTAMP)"
        ))?;
    }
    Ok(())
}

fn node_exists(
    conn: &GraphDbConnection<'_>,
    query: &str,
    params: Vec<(&str, GraphDbValue)>,
) -> Result<bool> {
    relationship_exists(conn, query, params)
}

fn relationship_exists(
    conn: &GraphDbConnection<'_>,
    query: &str,
    params: Vec<(&str, GraphDbValue)>,
) -> Result<bool> {
    let rows = graph_rows(conn, query, params)?;
    Ok(rows
        .first()
        .map(|row| graph_i64(row.first()).unwrap_or(0))
        .unwrap_or(0)
        > 0)
}

fn parse_blarify_timestamp(value: Option<&str>) -> Option<OffsetDateTime> {
    let parsed = DateTime::parse_from_rfc3339(value?).ok()?;
    OffsetDateTime::from_unix_timestamp(parsed.timestamp()).ok()
}
