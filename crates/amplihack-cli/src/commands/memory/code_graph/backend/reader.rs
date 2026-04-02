use super::super::super::backend::graph_db::{
    GraphDbConnection, GraphDbValue, graph_i64, graph_rows, graph_string,
};
use super::super::{
    CodeGraphContextClass, CodeGraphContextFile, CodeGraphContextFunction, CodeGraphContextPayload,
    CodeGraphEdgeEntry, CodeGraphNamedEntry, CodeGraphReaderBackend, CodeGraphSearchEntry,
    CodeGraphStats,
};
use super::schema::{GRAPH_MEMORY_FILE_LINK_TABLES, GRAPH_MEMORY_FUNCTION_LINK_TABLES};
use super::{ensure_memory_code_link_schema, open_graph_db_code_graph_db};
use anyhow::Result;
use std::path::Path;

pub(in crate::commands::memory::code_graph) fn open_code_graph_reader(
    path_override: Option<&Path>,
) -> Result<Box<dyn CodeGraphReaderBackend>> {
    Ok(Box::new(GraphDbCodeGraphReader::open(path_override)?))
}

struct GraphDbCodeGraphReader {
    handle: super::super::super::backend::graph_db::GraphDbHandle,
}

impl GraphDbCodeGraphReader {
    fn open(path_override: Option<&Path>) -> Result<Self> {
        Ok(Self {
            handle: open_graph_db_code_graph_db(path_override)?,
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&GraphDbConnection<'_>) -> Result<T>) -> Result<T> {
        self.handle
            .with_initialized_conn(ensure_memory_code_link_schema, f)
    }
}

impl CodeGraphReaderBackend for GraphDbCodeGraphReader {
    fn stats(&self) -> Result<CodeGraphStats> {
        self.with_conn(read_code_graph_stats_in_conn)
    }

    fn context_payload(&self, memory_id: &str) -> Result<CodeGraphContextPayload> {
        self.with_conn(|conn| query_code_context_in_conn(conn, memory_id))
    }

    fn files(&self, pattern: Option<&str>, limit: u32) -> Result<Vec<String>> {
        self.with_conn(|conn| list_code_files_in_conn(conn, pattern, limit))
    }

    fn functions(&self, file: Option<&str>, limit: u32) -> Result<Vec<CodeGraphNamedEntry>> {
        self.with_conn(|conn| list_code_functions_in_conn(conn, file, limit))
    }

    fn classes(&self, file: Option<&str>, limit: u32) -> Result<Vec<CodeGraphNamedEntry>> {
        self.with_conn(|conn| list_code_classes_in_conn(conn, file, limit))
    }

    fn search(&self, name: &str, limit: u32) -> Result<Vec<CodeGraphSearchEntry>> {
        self.with_conn(|conn| search_code_graph_in_conn(conn, name, limit))
    }

    fn callers(&self, name: &str, limit: u32) -> Result<Vec<CodeGraphEdgeEntry>> {
        self.with_conn(|conn| query_code_edges_in_conn(
            conn,
            "MATCH (caller:CodeFunction)-[:CALLS]->(callee:CodeFunction) WHERE callee.function_name CONTAINS $name RETURN caller.function_name, callee.function_name LIMIT $lim",
            name,
            limit,
        ))
    }

    fn callees(&self, name: &str, limit: u32) -> Result<Vec<CodeGraphEdgeEntry>> {
        self.with_conn(|conn| query_code_edges_in_conn(
            conn,
            "MATCH (caller:CodeFunction)-[:CALLS]->(callee:CodeFunction) WHERE caller.function_name CONTAINS $name RETURN caller.function_name, callee.function_name LIMIT $lim",
            name,
            limit,
        ))
    }
}

fn code_memory_link_counts(conn: &GraphDbConnection<'_>) -> Result<(i64, i64)> {
    ensure_memory_code_link_schema(conn)?;

    let file_links =
        GRAPH_MEMORY_FILE_LINK_TABLES
            .iter()
            .try_fold(0i64, |acc, (_, rel_table)| {
                Ok::<i64, anyhow::Error>(
                    acc + scalar_count(
                        conn,
                        &format!("MATCH ()-[r:{rel_table}]->(:CodeFile) RETURN COUNT(r)"),
                    )?,
                )
            })?;
    let function_links =
        GRAPH_MEMORY_FUNCTION_LINK_TABLES
            .iter()
            .try_fold(0i64, |acc, (_, rel_table)| {
                Ok::<i64, anyhow::Error>(
                    acc + scalar_count(
                        conn,
                        &format!("MATCH ()-[r:{rel_table}]->(:CodeFunction) RETURN COUNT(r)"),
                    )?,
                )
            })?;

    Ok((file_links, function_links))
}

fn scalar_count(conn: &GraphDbConnection<'_>, query: &str) -> Result<i64> {
    let rows = graph_rows(conn, query, vec![])?;
    graph_i64(rows.first().and_then(|row| row.first()))
}

fn read_code_graph_stats_in_conn(conn: &GraphDbConnection<'_>) -> Result<CodeGraphStats> {
    let (memory_file_links, memory_function_links) = code_memory_link_counts(conn)?;
    Ok(CodeGraphStats {
        files: scalar_count(conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)")?,
        classes: scalar_count(conn, "MATCH (c:CodeClass) RETURN COUNT(c)")?,
        functions: scalar_count(conn, "MATCH (f:CodeFunction) RETURN COUNT(f)")?,
        memory_file_links,
        memory_function_links,
    })
}

fn query_code_context_in_conn(
    conn: &GraphDbConnection<'_>,
    memory_id: &str,
) -> Result<CodeGraphContextPayload> {
    let Some((memory_type, file_rel, function_rel)) =
        resolve_memory_link_tables_in_conn(conn, memory_id)?
    else {
        return Ok(CodeGraphContextPayload {
            memory_id: memory_id.to_string(),
            ..Default::default()
        });
    };

    let files = graph_rows(
        conn,
        &format!(
            "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[:{file_rel}]->(cf:CodeFile) RETURN cf.file_path, cf.language, cf.size_bytes ORDER BY cf.file_path"
        ),
        vec![("memory_id", GraphDbValue::String(memory_id.to_string()))],
    )?;
    let functions = graph_rows(
        conn,
        &format!(
            "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[:{function_rel}]->(f:CodeFunction) RETURN f.function_name, f.signature, f.docstring, f.cyclomatic_complexity ORDER BY f.function_name"
        ),
        vec![("memory_id", GraphDbValue::String(memory_id.to_string()))],
    )?;
    let classes = graph_rows(
        conn,
        &format!(
            "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[:{function_rel}]->(f:CodeFunction)-[:METHOD_OF]->(c:CodeClass) RETURN DISTINCT c.class_name, c.fully_qualified_name, c.docstring ORDER BY c.class_name"
        ),
        vec![("memory_id", GraphDbValue::String(memory_id.to_string()))],
    )?;

    Ok(CodeGraphContextPayload {
        memory_id: memory_id.to_string(),
        files: files
            .iter()
            .map(|row| CodeGraphContextFile {
                kind: "file".to_string(),
                path: graph_string(row.first()).unwrap_or_default(),
                language: graph_string(row.get(1)).unwrap_or_default(),
                size_bytes: graph_i64(row.get(2)).unwrap_or_default(),
            })
            .collect(),
        functions: functions
            .iter()
            .map(|row| CodeGraphContextFunction {
                kind: "function".to_string(),
                name: graph_string(row.first()).unwrap_or_default(),
                signature: graph_string(row.get(1)).unwrap_or_default(),
                docstring: graph_string(row.get(2)).unwrap_or_default(),
                complexity: graph_i64(row.get(3)).unwrap_or_default(),
            })
            .collect(),
        classes: classes
            .iter()
            .map(|row| CodeGraphContextClass {
                kind: "class".to_string(),
                name: graph_string(row.first()).unwrap_or_default(),
                fully_qualified_name: graph_string(row.get(1)).unwrap_or_default(),
                docstring: graph_string(row.get(2)).unwrap_or_default(),
            })
            .collect(),
    })
}

fn resolve_memory_link_tables_in_conn(
    conn: &GraphDbConnection<'_>,
    memory_id: &str,
) -> Result<Option<(&'static str, &'static str, &'static str)>> {
    for ((memory_type, file_rel), (paired_type, function_rel)) in GRAPH_MEMORY_FILE_LINK_TABLES
        .iter()
        .zip(GRAPH_MEMORY_FUNCTION_LINK_TABLES.iter())
    {
        debug_assert_eq!(memory_type, paired_type);
        let rows = graph_rows(
            conn,
            &format!("MATCH (m:{memory_type} {{memory_id: $memory_id}}) RETURN COUNT(m)"),
            vec![("memory_id", GraphDbValue::String(memory_id.to_string()))],
        )?;
        if graph_i64(rows.first().and_then(|row| row.first()))? > 0 {
            return Ok(Some((memory_type, file_rel, function_rel)));
        }
    }

    Ok(None)
}

fn list_code_files_in_conn(
    conn: &GraphDbConnection<'_>,
    pattern: Option<&str>,
    limit: u32,
) -> Result<Vec<String>> {
    let rows = if let Some(pattern) = pattern {
        graph_rows(
            conn,
            "MATCH (cf:CodeFile) WHERE cf.file_path CONTAINS $pattern RETURN cf.file_path ORDER BY cf.file_path LIMIT $lim",
            vec![
                ("pattern", GraphDbValue::String(pattern.to_string())),
                ("lim", GraphDbValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        graph_rows(
            conn,
            "MATCH (cf:CodeFile) RETURN cf.file_path ORDER BY cf.file_path LIMIT $lim",
            vec![("lim", GraphDbValue::UInt64(u64::from(limit)))],
        )?
    };

    Ok(rows
        .iter()
        .map(|row| graph_string(row.first()).unwrap_or_default())
        .collect())
}

fn list_code_functions_in_conn(
    conn: &GraphDbConnection<'_>,
    file: Option<&str>,
    limit: u32,
) -> Result<Vec<CodeGraphNamedEntry>> {
    let rows = if let Some(file) = file {
        graph_rows(
            conn,
            "MATCH (f:CodeFunction)-[:DEFINED_IN]->(cf:CodeFile) WHERE cf.file_path CONTAINS $file AND NOT f.function_name CONTAINS '().(' RETURN f.function_name, cf.file_path ORDER BY cf.file_path, f.function_name LIMIT $lim",
            vec![
                ("file", GraphDbValue::String(file.to_string())),
                ("lim", GraphDbValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        graph_rows(
            conn,
            "MATCH (f:CodeFunction) WHERE NOT f.function_name CONTAINS '().(' RETURN f.function_name ORDER BY f.function_name LIMIT $lim",
            vec![("lim", GraphDbValue::UInt64(u64::from(limit)))],
        )?
    };

    Ok(rows
        .iter()
        .map(|row| CodeGraphNamedEntry {
            name: graph_string(row.first()).unwrap_or_default(),
            file: row
                .get(1)
                .map(|value| graph_string(Some(value)).unwrap_or_default()),
        })
        .collect())
}

fn list_code_classes_in_conn(
    conn: &GraphDbConnection<'_>,
    file: Option<&str>,
    limit: u32,
) -> Result<Vec<CodeGraphNamedEntry>> {
    let rows = if let Some(file) = file {
        graph_rows(
            conn,
            "MATCH (c:CodeClass)-[:CLASS_DEFINED_IN]->(cf:CodeFile) WHERE cf.file_path CONTAINS $file RETURN c.class_name, cf.file_path ORDER BY cf.file_path, c.class_name LIMIT $lim",
            vec![
                ("file", GraphDbValue::String(file.to_string())),
                ("lim", GraphDbValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        graph_rows(
            conn,
            "MATCH (c:CodeClass) RETURN c.class_name ORDER BY c.class_name LIMIT $lim",
            vec![("lim", GraphDbValue::UInt64(u64::from(limit)))],
        )?
    };

    Ok(rows
        .iter()
        .map(|row| CodeGraphNamedEntry {
            name: graph_string(row.first()).unwrap_or_default(),
            file: row
                .get(1)
                .map(|value| graph_string(Some(value)).unwrap_or_default()),
        })
        .collect())
}

fn search_code_graph_in_conn(
    conn: &GraphDbConnection<'_>,
    name: &str,
    limit: u32,
) -> Result<Vec<CodeGraphSearchEntry>> {
    let searches = [
        (
            "function",
            "MATCH (f:CodeFunction) WHERE f.function_name CONTAINS $name AND NOT f.function_name CONTAINS '().(' RETURN f.function_name LIMIT $lim",
        ),
        (
            "class",
            "MATCH (c:CodeClass) WHERE c.class_name CONTAINS $name RETURN c.class_name LIMIT $lim",
        ),
        (
            "file",
            "MATCH (cf:CodeFile) WHERE cf.file_path CONTAINS $name RETURN cf.file_path LIMIT $lim",
        ),
    ];

    let mut payload = Vec::new();
    for (kind, query) in searches {
        let rows = graph_rows(
            conn,
            query,
            vec![
                ("name", GraphDbValue::String(name.to_string())),
                ("lim", GraphDbValue::UInt64(u64::from(limit))),
            ],
        )?;
        payload.extend(rows.into_iter().map(|row| CodeGraphSearchEntry {
            kind: kind.to_string(),
            name: graph_string(row.first()).unwrap_or_default(),
        }));
    }

    Ok(payload)
}

fn query_code_edges_in_conn(
    conn: &GraphDbConnection<'_>,
    query: &str,
    name: &str,
    limit: u32,
) -> Result<Vec<CodeGraphEdgeEntry>> {
    let rows = graph_rows(
        conn,
        query,
        vec![
            ("name", GraphDbValue::String(name.to_string())),
            ("lim", GraphDbValue::UInt64(u64::from(limit))),
        ],
    )?;

    Ok(rows
        .iter()
        .map(|row| CodeGraphEdgeEntry {
            caller: graph_string(row.first()).unwrap_or_default(),
            callee: graph_string(row.get(1)).unwrap_or_default(),
        })
        .collect())
}
