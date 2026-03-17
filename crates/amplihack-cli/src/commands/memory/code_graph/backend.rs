use super::super::backend::graph_db::{
    graph_i64, graph_rows, graph_string, init_graph_backend_schema,
};
use super::*;
use chrono::DateTime;
use kuzu::{
    Connection as KuzuConnection, Database as KuzuDatabase, SystemConfig, Value as KuzuValue,
};
use std::fs;
use std::path::Path;
use time::OffsetDateTime;

const GRAPH_CODE_GRAPH_SCHEMA: &[&str] = &[
    r#"CREATE NODE TABLE IF NOT EXISTS CodeFile(
        file_id STRING,
        file_path STRING,
        language STRING,
        size_bytes INT64,
        last_modified TIMESTAMP,
        created_at TIMESTAMP,
        metadata STRING,
        PRIMARY KEY (file_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS CodeClass(
        class_id STRING,
        class_name STRING,
        fully_qualified_name STRING,
        file_path STRING,
        line_number INT64,
        docstring STRING,
        is_abstract BOOL,
        created_at TIMESTAMP,
        metadata STRING,
        PRIMARY KEY (class_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS CodeFunction(
        function_id STRING,
        function_name STRING,
        fully_qualified_name STRING,
        signature STRING,
        file_path STRING,
        line_number INT64,
        parameters STRING,
        return_type STRING,
        docstring STRING,
        is_async BOOL,
        cyclomatic_complexity INT64,
        created_at TIMESTAMP,
        metadata STRING,
        PRIMARY KEY (function_id)
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS DEFINED_IN(
        FROM CodeFunction TO CodeFile,
        line_number INT64,
        end_line INT64
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS CLASS_DEFINED_IN(
        FROM CodeClass TO CodeFile,
        line_number INT64
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS METHOD_OF(
        FROM CodeFunction TO CodeClass,
        method_type STRING,
        visibility STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS CALLS(
        FROM CodeFunction TO CodeFunction,
        call_count INT64,
        context STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS INHERITS(
        FROM CodeClass TO CodeClass,
        inheritance_order INT64,
        inheritance_type STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS REFERENCES_CLASS(
        FROM CodeFunction TO CodeClass,
        reference_type STRING,
        context STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS IMPORTS(
        FROM CodeFile TO CodeFile,
        import_type STRING,
        alias STRING
    )"#,
];

pub(super) const GRAPH_MEMORY_FILE_LINK_TABLES: &[(&str, &str)] = &[
    ("EpisodicMemory", "RELATES_TO_FILE_EPISODIC"),
    ("SemanticMemory", "RELATES_TO_FILE_SEMANTIC"),
    ("ProceduralMemory", "RELATES_TO_FILE_PROCEDURAL"),
    ("ProspectiveMemory", "RELATES_TO_FILE_PROSPECTIVE"),
    ("WorkingMemory", "RELATES_TO_FILE_WORKING"),
];

pub(super) const GRAPH_MEMORY_FUNCTION_LINK_TABLES: &[(&str, &str)] = &[
    ("EpisodicMemory", "RELATES_TO_FUNCTION_EPISODIC"),
    ("SemanticMemory", "RELATES_TO_FUNCTION_SEMANTIC"),
    ("ProceduralMemory", "RELATES_TO_FUNCTION_PROCEDURAL"),
    ("ProspectiveMemory", "RELATES_TO_FUNCTION_PROSPECTIVE"),
    ("WorkingMemory", "RELATES_TO_FUNCTION_WORKING"),
];

pub(super) fn open_code_graph_reader(
    path_override: Option<&Path>,
) -> Result<Box<dyn CodeGraphReaderBackend>> {
    Ok(Box::new(GraphDbCodeGraphReader::open(path_override)?))
}

struct GraphDbCodeGraphReader {
    db: KuzuDatabase,
}

impl GraphDbCodeGraphReader {
    fn open(path_override: Option<&Path>) -> Result<Self> {
        Ok(Self {
            db: open_graph_db_code_graph_db(path_override)?,
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&KuzuConnection<'_>) -> Result<T>) -> Result<T> {
        let conn = KuzuConnection::new(&self.db)?;
        ensure_memory_code_link_schema(&conn)?;
        f(&conn)
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

pub(super) fn open_code_graph_writer(
    path_override: Option<&Path>,
) -> Result<Box<dyn CodeGraphWriterBackend>> {
    Ok(Box::new(GraphDbCodeGraphWriter::open(path_override)?))
}

struct GraphDbCodeGraphWriter {
    db: KuzuDatabase,
}

impl GraphDbCodeGraphWriter {
    fn open(path_override: Option<&Path>) -> Result<Self> {
        Ok(Self {
            db: open_graph_db_code_graph_db(path_override)?,
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&KuzuConnection<'_>) -> Result<T>) -> Result<T> {
        let conn = KuzuConnection::new(&self.db)?;
        ensure_memory_code_link_schema(&conn)?;
        f(&conn)
    }
}

impl CodeGraphWriterBackend for GraphDbCodeGraphWriter {
    fn import_blarify_output(&self, payload: &BlarifyOutput) -> Result<CodeGraphImportCounts> {
        self.with_conn(|conn| {
            let counts = CodeGraphImportCounts {
                files: import_files(conn, &payload.files)?,
                classes: import_classes(conn, &payload.classes)?,
                functions: import_functions(conn, &payload.functions)?,
                imports: import_imports(conn, &payload.imports)?,
                relationships: import_relationships(conn, &payload.relationships)?,
            };
            let linked_memories = link_memories_to_code_files_in_conn(conn)?;
            if linked_memories > 0 {
                tracing::info!(
                    count = linked_memories,
                    "linked existing memories to code graph after import"
                );
            }
            Ok(counts)
        })
    }
}

pub(super) fn open_graph_db_code_graph_db(path_override: Option<&Path>) -> Result<KuzuDatabase> {
    let path = match path_override {
        Some(path) => path.to_path_buf(),
        None => default_code_graph_db_path()?,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let db = KuzuDatabase::new(&path, SystemConfig::default())?;
    enforce_db_permissions(&path)?;
    Ok(db)
}

#[cfg(test)]
pub(crate) fn with_test_code_graph_conn<T>(
    path_override: Option<&Path>,
    f: impl FnOnce(&KuzuConnection<'_>) -> Result<T>,
) -> Result<T> {
    let db = open_graph_db_code_graph_db(path_override)?;
    let conn = KuzuConnection::new(&db)?;
    ensure_memory_code_link_schema(&conn)?;
    f(&conn)
}

#[cfg(test)]
pub(crate) fn initialize_test_code_graph_db(path_override: Option<&Path>) -> Result<()> {
    with_test_code_graph_conn(path_override, |_| Ok(()))
}

fn init_graph_db_code_graph_schema(conn: &KuzuConnection<'_>) -> Result<()> {
    for statement in GRAPH_CODE_GRAPH_SCHEMA {
        conn.query(statement)?;
    }
    Ok(())
}

pub(super) fn ensure_memory_code_link_schema(conn: &KuzuConnection<'_>) -> Result<()> {
    init_graph_backend_schema(conn)?;
    init_graph_db_code_graph_schema(conn)?;
    for (memory_type, rel_table) in GRAPH_MEMORY_FILE_LINK_TABLES {
        conn.query(&format!(
            "CREATE REL TABLE IF NOT EXISTS {rel_table}(FROM {memory_type} TO CodeFile, relevance_score DOUBLE, context STRING, timestamp TIMESTAMP)"
        ))?;
    }
    for (memory_type, rel_table) in GRAPH_MEMORY_FUNCTION_LINK_TABLES {
        conn.query(&format!(
            "CREATE REL TABLE IF NOT EXISTS {rel_table}(FROM {memory_type} TO CodeFunction, relevance_score DOUBLE, context STRING, timestamp TIMESTAMP)"
        ))?;
    }
    Ok(())
}

fn code_memory_link_counts(conn: &KuzuConnection<'_>) -> Result<(i64, i64)> {
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

fn scalar_count(conn: &KuzuConnection<'_>, query: &str) -> Result<i64> {
    let rows = graph_rows(conn, query, vec![])?;
    graph_i64(rows.first().and_then(|row| row.first()))
}

fn read_code_graph_stats_in_conn(conn: &KuzuConnection<'_>) -> Result<CodeGraphStats> {
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
    conn: &KuzuConnection<'_>,
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
        vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
    )?;
    let functions = graph_rows(
        conn,
        &format!(
            "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[:{function_rel}]->(f:CodeFunction) RETURN f.function_name, f.signature, f.docstring, f.cyclomatic_complexity ORDER BY f.function_name"
        ),
        vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
    )?;
    let classes = graph_rows(
        conn,
        &format!(
            "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[:{function_rel}]->(f:CodeFunction)-[:METHOD_OF]->(c:CodeClass) RETURN DISTINCT c.class_name, c.fully_qualified_name, c.docstring ORDER BY c.class_name"
        ),
        vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
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
    conn: &KuzuConnection<'_>,
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
            vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
        )?;
        if graph_i64(rows.first().and_then(|row| row.first()))? > 0 {
            return Ok(Some((memory_type, file_rel, function_rel)));
        }
    }

    Ok(None)
}

fn list_code_files_in_conn(
    conn: &KuzuConnection<'_>,
    pattern: Option<&str>,
    limit: u32,
) -> Result<Vec<String>> {
    let rows = if let Some(pattern) = pattern {
        graph_rows(
            conn,
            "MATCH (cf:CodeFile) WHERE cf.file_path CONTAINS $pattern RETURN cf.file_path ORDER BY cf.file_path LIMIT $lim",
            vec![
                ("pattern", KuzuValue::String(pattern.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        graph_rows(
            conn,
            "MATCH (cf:CodeFile) RETURN cf.file_path ORDER BY cf.file_path LIMIT $lim",
            vec![("lim", KuzuValue::UInt64(u64::from(limit)))],
        )?
    };

    Ok(rows
        .iter()
        .map(|row| graph_string(row.first()).unwrap_or_default())
        .collect())
}

fn list_code_functions_in_conn(
    conn: &KuzuConnection<'_>,
    file: Option<&str>,
    limit: u32,
) -> Result<Vec<CodeGraphNamedEntry>> {
    let rows = if let Some(file) = file {
        graph_rows(
            conn,
            "MATCH (f:CodeFunction)-[:DEFINED_IN]->(cf:CodeFile) WHERE cf.file_path CONTAINS $file AND NOT f.function_name CONTAINS '().(' RETURN f.function_name, cf.file_path ORDER BY cf.file_path, f.function_name LIMIT $lim",
            vec![
                ("file", KuzuValue::String(file.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        graph_rows(
            conn,
            "MATCH (f:CodeFunction) WHERE NOT f.function_name CONTAINS '().(' RETURN f.function_name ORDER BY f.function_name LIMIT $lim",
            vec![("lim", KuzuValue::UInt64(u64::from(limit)))],
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
    conn: &KuzuConnection<'_>,
    file: Option<&str>,
    limit: u32,
) -> Result<Vec<CodeGraphNamedEntry>> {
    let rows = if let Some(file) = file {
        graph_rows(
            conn,
            "MATCH (c:CodeClass)-[:CLASS_DEFINED_IN]->(cf:CodeFile) WHERE cf.file_path CONTAINS $file RETURN c.class_name, cf.file_path ORDER BY cf.file_path, c.class_name LIMIT $lim",
            vec![
                ("file", KuzuValue::String(file.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        graph_rows(
            conn,
            "MATCH (c:CodeClass) RETURN c.class_name ORDER BY c.class_name LIMIT $lim",
            vec![("lim", KuzuValue::UInt64(u64::from(limit)))],
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
    conn: &KuzuConnection<'_>,
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
                ("name", KuzuValue::String(name.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
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
    conn: &KuzuConnection<'_>,
    query: &str,
    name: &str,
    limit: u32,
) -> Result<Vec<CodeGraphEdgeEntry>> {
    let rows = graph_rows(
        conn,
        query,
        vec![
            ("name", KuzuValue::String(name.to_string())),
            ("lim", KuzuValue::UInt64(u64::from(limit))),
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

fn import_files(conn: &KuzuConnection<'_>, files: &[BlarifyFile]) -> Result<usize> {
    let now = OffsetDateTime::now_utc();
    let mut imported = 0usize;

    for file in files {
        if file.path.trim().is_empty() {
            continue;
        }

        let last_modified = parse_blarify_timestamp(file.last_modified.as_deref()).unwrap_or(now);
        let exists = node_exists(
            conn,
            "MATCH (cf:CodeFile {file_id: $file_id}) RETURN COUNT(cf)",
            vec![("file_id", KuzuValue::String(file.path.clone()))],
        )?;

        if exists {
            graph_rows(
                conn,
                "MATCH (cf:CodeFile {file_id: $file_id}) SET cf.file_path = $file_path, cf.language = $language, cf.size_bytes = $size_bytes, cf.last_modified = $last_modified",
                vec![
                    ("file_id", KuzuValue::String(file.path.clone())),
                    ("file_path", KuzuValue::String(file.path.clone())),
                    ("language", KuzuValue::String(file.language.clone())),
                    ("size_bytes", KuzuValue::Int64(file.lines_of_code)),
                    ("last_modified", KuzuValue::Timestamp(last_modified)),
                ],
            )?;
        } else {
            graph_rows(
                conn,
                "CREATE (cf:CodeFile {file_id: $file_id, file_path: $file_path, language: $language, size_bytes: $size_bytes, last_modified: $last_modified, created_at: $created_at, metadata: $metadata})",
                vec![
                    ("file_id", KuzuValue::String(file.path.clone())),
                    ("file_path", KuzuValue::String(file.path.clone())),
                    ("language", KuzuValue::String(file.language.clone())),
                    ("size_bytes", KuzuValue::Int64(file.lines_of_code)),
                    ("last_modified", KuzuValue::Timestamp(last_modified)),
                    ("created_at", KuzuValue::Timestamp(now)),
                    ("metadata", KuzuValue::String("{}".to_string())),
                ],
            )?;
        }

        imported += 1;
    }

    Ok(imported)
}

fn import_classes(conn: &KuzuConnection<'_>, classes: &[BlarifyClass]) -> Result<usize> {
    let now = OffsetDateTime::now_utc();
    let mut imported = 0usize;

    for class in classes {
        if class.id.trim().is_empty() {
            continue;
        }

        let metadata = serde_json::json!({ "line_number": class.line_number }).to_string();
        let exists = node_exists(
            conn,
            "MATCH (c:CodeClass {class_id: $class_id}) RETURN COUNT(c)",
            vec![("class_id", KuzuValue::String(class.id.clone()))],
        )?;

        if exists {
            graph_rows(
                conn,
                "MATCH (c:CodeClass {class_id: $class_id}) SET c.class_name = $class_name, c.fully_qualified_name = $fully_qualified_name, c.file_path = $file_path, c.line_number = $line_number, c.docstring = $docstring, c.is_abstract = $is_abstract, c.metadata = $metadata",
                vec![
                    ("class_id", KuzuValue::String(class.id.clone())),
                    ("class_name", KuzuValue::String(class.name.clone())),
                    ("fully_qualified_name", KuzuValue::String(class.id.clone())),
                    ("file_path", KuzuValue::String(class.file_path.clone())),
                    ("line_number", KuzuValue::Int64(class.line_number)),
                    ("docstring", KuzuValue::String(class.docstring.clone())),
                    ("is_abstract", KuzuValue::Bool(class.is_abstract)),
                    ("metadata", KuzuValue::String(metadata.clone())),
                ],
            )?;
        } else {
            graph_rows(
                conn,
                "CREATE (c:CodeClass {class_id: $class_id, class_name: $class_name, fully_qualified_name: $fully_qualified_name, file_path: $file_path, line_number: $line_number, docstring: $docstring, is_abstract: $is_abstract, created_at: $created_at, metadata: $metadata})",
                vec![
                    ("class_id", KuzuValue::String(class.id.clone())),
                    ("class_name", KuzuValue::String(class.name.clone())),
                    ("fully_qualified_name", KuzuValue::String(class.id.clone())),
                    ("file_path", KuzuValue::String(class.file_path.clone())),
                    ("line_number", KuzuValue::Int64(class.line_number)),
                    ("docstring", KuzuValue::String(class.docstring.clone())),
                    ("is_abstract", KuzuValue::Bool(class.is_abstract)),
                    ("created_at", KuzuValue::Timestamp(now)),
                    ("metadata", KuzuValue::String(metadata)),
                ],
            )?;
        }

        if !class.file_path.is_empty()
            && !relationship_exists(
                conn,
                "MATCH (c:CodeClass {class_id: $class_id})-[r:CLASS_DEFINED_IN]->(cf:CodeFile {file_id: $file_id}) RETURN COUNT(r)",
                vec![
                    ("class_id", KuzuValue::String(class.id.clone())),
                    ("file_id", KuzuValue::String(class.file_path.clone())),
                ],
            )?
        {
            graph_rows(
                conn,
                "MATCH (c:CodeClass {class_id: $class_id}) MATCH (cf:CodeFile {file_id: $file_id}) CREATE (c)-[:CLASS_DEFINED_IN {line_number: $line_number}]->(cf)",
                vec![
                    ("class_id", KuzuValue::String(class.id.clone())),
                    ("file_id", KuzuValue::String(class.file_path.clone())),
                    ("line_number", KuzuValue::Int64(class.line_number)),
                ],
            )?;
        }

        imported += 1;
    }

    Ok(imported)
}

fn import_functions(conn: &KuzuConnection<'_>, functions: &[BlarifyFunction]) -> Result<usize> {
    let now = OffsetDateTime::now_utc();
    let mut imported = 0usize;

    for function in functions {
        if function.id.trim().is_empty() {
            continue;
        }

        let parameters_json = serde_json::to_string(&function.parameters)?;
        let signature = format!("{}({})", function.name, function.parameters.join(", "));
        let metadata = serde_json::json!({
            "line_number": function.line_number,
            "parameters": function.parameters,
            "return_type": function.return_type,
        })
        .to_string();
        let exists = node_exists(
            conn,
            "MATCH (f:CodeFunction {function_id: $function_id}) RETURN COUNT(f)",
            vec![("function_id", KuzuValue::String(function.id.clone()))],
        )?;

        if exists {
            graph_rows(
                conn,
                "MATCH (f:CodeFunction {function_id: $function_id}) SET f.function_name = $function_name, f.fully_qualified_name = $fully_qualified_name, f.signature = $signature, f.file_path = $file_path, f.line_number = $line_number, f.parameters = $parameters, f.return_type = $return_type, f.docstring = $docstring, f.is_async = $is_async, f.cyclomatic_complexity = $cyclomatic_complexity, f.metadata = $metadata",
                vec![
                    ("function_id", KuzuValue::String(function.id.clone())),
                    ("function_name", KuzuValue::String(function.name.clone())),
                    (
                        "fully_qualified_name",
                        KuzuValue::String(function.id.clone()),
                    ),
                    ("signature", KuzuValue::String(signature.clone())),
                    ("file_path", KuzuValue::String(function.file_path.clone())),
                    ("line_number", KuzuValue::Int64(function.line_number)),
                    ("parameters", KuzuValue::String(parameters_json.clone())),
                    (
                        "return_type",
                        KuzuValue::String(function.return_type.clone()),
                    ),
                    ("docstring", KuzuValue::String(function.docstring.clone())),
                    ("is_async", KuzuValue::Bool(function.is_async)),
                    (
                        "cyclomatic_complexity",
                        KuzuValue::Int64(function.complexity),
                    ),
                    ("metadata", KuzuValue::String(metadata.clone())),
                ],
            )?;
        } else {
            graph_rows(
                conn,
                "CREATE (f:CodeFunction {function_id: $function_id, function_name: $function_name, fully_qualified_name: $fully_qualified_name, signature: $signature, file_path: $file_path, line_number: $line_number, parameters: $parameters, return_type: $return_type, docstring: $docstring, is_async: $is_async, cyclomatic_complexity: $cyclomatic_complexity, created_at: $created_at, metadata: $metadata})",
                vec![
                    ("function_id", KuzuValue::String(function.id.clone())),
                    ("function_name", KuzuValue::String(function.name.clone())),
                    (
                        "fully_qualified_name",
                        KuzuValue::String(function.id.clone()),
                    ),
                    ("signature", KuzuValue::String(signature)),
                    ("file_path", KuzuValue::String(function.file_path.clone())),
                    ("line_number", KuzuValue::Int64(function.line_number)),
                    ("parameters", KuzuValue::String(parameters_json)),
                    (
                        "return_type",
                        KuzuValue::String(function.return_type.clone()),
                    ),
                    ("docstring", KuzuValue::String(function.docstring.clone())),
                    ("is_async", KuzuValue::Bool(function.is_async)),
                    (
                        "cyclomatic_complexity",
                        KuzuValue::Int64(function.complexity),
                    ),
                    ("created_at", KuzuValue::Timestamp(now)),
                    ("metadata", KuzuValue::String(metadata)),
                ],
            )?;
        }

        if !function.file_path.is_empty()
            && !relationship_exists(
                conn,
                "MATCH (f:CodeFunction {function_id: $function_id})-[r:DEFINED_IN]->(cf:CodeFile {file_id: $file_id}) RETURN COUNT(r)",
                vec![
                    ("function_id", KuzuValue::String(function.id.clone())),
                    ("file_id", KuzuValue::String(function.file_path.clone())),
                ],
            )?
        {
            graph_rows(
                conn,
                "MATCH (f:CodeFunction {function_id: $function_id}) MATCH (cf:CodeFile {file_id: $file_id}) CREATE (f)-[:DEFINED_IN {line_number: $line_number, end_line: $end_line}]->(cf)",
                vec![
                    ("function_id", KuzuValue::String(function.id.clone())),
                    ("file_id", KuzuValue::String(function.file_path.clone())),
                    ("line_number", KuzuValue::Int64(function.line_number)),
                    ("end_line", KuzuValue::Int64(function.line_number)),
                ],
            )?;
        }

        if let Some(class_id) = function.class_id.as_ref()
            && !relationship_exists(
                conn,
                "MATCH (f:CodeFunction {function_id: $function_id})-[r:METHOD_OF]->(c:CodeClass {class_id: $class_id}) RETURN COUNT(r)",
                vec![
                    ("function_id", KuzuValue::String(function.id.clone())),
                    ("class_id", KuzuValue::String(class_id.clone())),
                ],
            )?
        {
            graph_rows(
                conn,
                "MATCH (f:CodeFunction {function_id: $function_id}) MATCH (c:CodeClass {class_id: $class_id}) CREATE (f)-[:METHOD_OF {method_type: $method_type, visibility: $visibility}]->(c)",
                vec![
                    ("function_id", KuzuValue::String(function.id.clone())),
                    ("class_id", KuzuValue::String(class_id.clone())),
                    ("method_type", KuzuValue::String("instance".to_string())),
                    ("visibility", KuzuValue::String("public".to_string())),
                ],
            )?;
        }

        imported += 1;
    }

    Ok(imported)
}

fn import_imports(conn: &KuzuConnection<'_>, imports: &[BlarifyImport]) -> Result<usize> {
    let mut imported = 0usize;

    for import in imports {
        if import.source_file.trim().is_empty() || import.target_file.trim().is_empty() {
            continue;
        }

        if relationship_exists(
            conn,
            "MATCH (source:CodeFile {file_id: $source_file})-[r:IMPORTS]->(target:CodeFile {file_id: $target_file}) WHERE r.import_type = $import_type RETURN COUNT(r)",
            vec![
                ("source_file", KuzuValue::String(import.source_file.clone())),
                ("target_file", KuzuValue::String(import.target_file.clone())),
                ("import_type", KuzuValue::String(import.symbol.clone())),
            ],
        )? {
            continue;
        }

        graph_rows(
            conn,
            "MATCH (source:CodeFile {file_id: $source_file}) MATCH (target:CodeFile {file_id: $target_file}) CREATE (source)-[:IMPORTS {import_type: $import_type, alias: $alias}]->(target)",
            vec![
                ("source_file", KuzuValue::String(import.source_file.clone())),
                ("target_file", KuzuValue::String(import.target_file.clone())),
                ("import_type", KuzuValue::String(import.symbol.clone())),
                (
                    "alias",
                    KuzuValue::String(import.alias.clone().unwrap_or_default()),
                ),
            ],
        )?;
        imported += 1;
    }

    Ok(imported)
}

fn import_relationships(
    conn: &KuzuConnection<'_>,
    relationships: &[BlarifyRelationship],
) -> Result<usize> {
    let mut imported = 0usize;

    for relationship in relationships {
        if relationship.source_id.trim().is_empty() || relationship.target_id.trim().is_empty() {
            continue;
        }

        imported += match relationship.relationship_type.as_str() {
            "CALLS" => {
                create_calls_relationship(conn, &relationship.source_id, &relationship.target_id)?
            }
            "INHERITS" => create_inherits_relationship(
                conn,
                &relationship.source_id,
                &relationship.target_id,
            )?,
            "REFERENCES" => create_references_relationship(
                conn,
                &relationship.source_id,
                &relationship.target_id,
            )?,
            _ => 0,
        };
    }

    Ok(imported)
}

fn normalize_match_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn link_memories_to_code_files_in_conn(conn: &KuzuConnection<'_>) -> Result<usize> {
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
                vec![("file_path", KuzuValue::String(file_path))],
            )?;

            for file_row in matching_files {
                let file_id = graph_string(file_row.first())?;
                let existing = graph_rows(
                    conn,
                    &format!(
                        "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[r:{rel_table}]->(cf:CodeFile {{file_id: $file_id}}) RETURN COUNT(r)"
                    ),
                    vec![
                        ("memory_id", KuzuValue::String(memory_id.clone())),
                        ("file_id", KuzuValue::String(file_id.clone())),
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
                        ("memory_id", KuzuValue::String(memory_id.clone())),
                        ("file_id", KuzuValue::String(file_id)),
                        ("relevance_score", KuzuValue::Double(1.0)),
                        (
                            "context",
                            KuzuValue::String("metadata_file_match".to_string()),
                        ),
                        ("timestamp", KuzuValue::Timestamp(now)),
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
                vec![("content", KuzuValue::String(content))],
            )?;

            for function_row in matching_functions {
                let function_id = graph_string(function_row.first())?;
                let existing = graph_rows(
                    conn,
                    &format!(
                        "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[r:{rel_table}]->(f:CodeFunction {{function_id: $function_id}}) RETURN COUNT(r)"
                    ),
                    vec![
                        ("memory_id", KuzuValue::String(memory_id.clone())),
                        ("function_id", KuzuValue::String(function_id.clone())),
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
                        ("memory_id", KuzuValue::String(memory_id.clone())),
                        ("function_id", KuzuValue::String(function_id)),
                        ("relevance_score", KuzuValue::Double(0.8)),
                        (
                            "context",
                            KuzuValue::String("content_name_match".to_string()),
                        ),
                        ("timestamp", KuzuValue::Timestamp(now)),
                    ],
                )?;
                created += 1;
            }
        }
    }

    Ok(created)
}

fn create_calls_relationship(
    conn: &KuzuConnection<'_>,
    source_id: &str,
    target_id: &str,
) -> Result<usize> {
    if relationship_exists(
        conn,
        "MATCH (source:CodeFunction {function_id: $source_id})-[r:CALLS]->(target:CodeFunction {function_id: $target_id}) RETURN COUNT(r)",
        vec![
            ("source_id", KuzuValue::String(source_id.to_string())),
            ("target_id", KuzuValue::String(target_id.to_string())),
        ],
    )? {
        return Ok(0);
    }

    graph_rows(
        conn,
        "MATCH (source:CodeFunction {function_id: $source_id}) MATCH (target:CodeFunction {function_id: $target_id}) CREATE (source)-[:CALLS {call_count: $call_count, context: $context}]->(target)",
        vec![
            ("source_id", KuzuValue::String(source_id.to_string())),
            ("target_id", KuzuValue::String(target_id.to_string())),
            ("call_count", KuzuValue::Int64(1)),
            ("context", KuzuValue::String(String::new())),
        ],
    )?;
    Ok(1)
}

fn create_inherits_relationship(
    conn: &KuzuConnection<'_>,
    source_id: &str,
    target_id: &str,
) -> Result<usize> {
    if relationship_exists(
        conn,
        "MATCH (source:CodeClass {class_id: $source_id})-[r:INHERITS]->(target:CodeClass {class_id: $target_id}) RETURN COUNT(r)",
        vec![
            ("source_id", KuzuValue::String(source_id.to_string())),
            ("target_id", KuzuValue::String(target_id.to_string())),
        ],
    )? {
        return Ok(0);
    }

    graph_rows(
        conn,
        "MATCH (source:CodeClass {class_id: $source_id}) MATCH (target:CodeClass {class_id: $target_id}) CREATE (source)-[:INHERITS {inheritance_order: $inheritance_order, inheritance_type: $inheritance_type}]->(target)",
        vec![
            ("source_id", KuzuValue::String(source_id.to_string())),
            ("target_id", KuzuValue::String(target_id.to_string())),
            ("inheritance_order", KuzuValue::Int64(0)),
            ("inheritance_type", KuzuValue::String("single".to_string())),
        ],
    )?;
    Ok(1)
}

fn create_references_relationship(
    conn: &KuzuConnection<'_>,
    source_id: &str,
    target_id: &str,
) -> Result<usize> {
    if relationship_exists(
        conn,
        "MATCH (source:CodeFunction {function_id: $source_id})-[r:REFERENCES_CLASS]->(target:CodeClass {class_id: $target_id}) RETURN COUNT(r)",
        vec![
            ("source_id", KuzuValue::String(source_id.to_string())),
            ("target_id", KuzuValue::String(target_id.to_string())),
        ],
    )? {
        return Ok(0);
    }

    graph_rows(
        conn,
        "MATCH (source:CodeFunction {function_id: $source_id}) MATCH (target:CodeClass {class_id: $target_id}) CREATE (source)-[:REFERENCES_CLASS {reference_type: $reference_type, context: $context}]->(target)",
        vec![
            ("source_id", KuzuValue::String(source_id.to_string())),
            ("target_id", KuzuValue::String(target_id.to_string())),
            ("reference_type", KuzuValue::String("usage".to_string())),
            ("context", KuzuValue::String(String::new())),
        ],
    )?;
    Ok(1)
}

fn node_exists(
    conn: &KuzuConnection<'_>,
    query: &str,
    params: Vec<(&str, KuzuValue)>,
) -> Result<bool> {
    relationship_exists(conn, query, params)
}

fn relationship_exists(
    conn: &KuzuConnection<'_>,
    query: &str,
    params: Vec<(&str, KuzuValue)>,
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
