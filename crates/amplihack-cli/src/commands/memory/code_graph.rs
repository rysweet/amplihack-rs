//! Native blarify JSON → Kuzu code-graph import.

use super::{init_kuzu_backend_schema, kuzu_i64, kuzu_rows, kuzu_string};
use anyhow::{Context, Result, bail};
use chrono::DateTime;
use kuzu::{
    Connection as KuzuConnection, Database as KuzuDatabase, SystemConfig, Value as KuzuValue,
};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

const KUZU_CODE_GRAPH_SCHEMA: &[&str] = &[
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

const KUZU_MEMORY_FILE_LINK_TABLES: &[(&str, &str)] = &[
    ("EpisodicMemory", "RELATES_TO_FILE_EPISODIC"),
    ("SemanticMemory", "RELATES_TO_FILE_SEMANTIC"),
    ("ProceduralMemory", "RELATES_TO_FILE_PROCEDURAL"),
    ("ProspectiveMemory", "RELATES_TO_FILE_PROSPECTIVE"),
    ("WorkingMemory", "RELATES_TO_FILE_WORKING"),
];

const KUZU_MEMORY_FUNCTION_LINK_TABLES: &[(&str, &str)] = &[
    ("EpisodicMemory", "RELATES_TO_FUNCTION_EPISODIC"),
    ("SemanticMemory", "RELATES_TO_FUNCTION_SEMANTIC"),
    ("ProceduralMemory", "RELATES_TO_FUNCTION_PROCEDURAL"),
    ("ProspectiveMemory", "RELATES_TO_FUNCTION_PROSPECTIVE"),
    ("WorkingMemory", "RELATES_TO_FUNCTION_WORKING"),
];

const BLARIFY_JSON_MAX_BYTES: u64 = 500 * 1024 * 1024;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct BlarifyOutput {
    #[serde(default)]
    files: Vec<BlarifyFile>,
    #[serde(default)]
    classes: Vec<BlarifyClass>,
    #[serde(default)]
    functions: Vec<BlarifyFunction>,
    #[serde(default)]
    imports: Vec<BlarifyImport>,
    #[serde(default)]
    relationships: Vec<BlarifyRelationship>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BlarifyFile {
    #[serde(default)]
    path: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    lines_of_code: i64,
    #[serde(default)]
    last_modified: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BlarifyClass {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    file_path: String,
    #[serde(default)]
    line_number: i64,
    #[serde(default)]
    docstring: String,
    #[serde(default)]
    is_abstract: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BlarifyFunction {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    file_path: String,
    #[serde(default)]
    line_number: i64,
    #[serde(default)]
    docstring: String,
    #[serde(default)]
    parameters: Vec<String>,
    #[serde(default)]
    return_type: String,
    #[serde(default)]
    is_async: bool,
    #[serde(default)]
    complexity: i64,
    #[serde(default)]
    class_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BlarifyImport {
    #[serde(default)]
    source_file: String,
    #[serde(default)]
    target_file: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    alias: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BlarifyRelationship {
    #[serde(default, rename = "type")]
    relationship_type: String,
    #[serde(default)]
    source_id: String,
    #[serde(default)]
    target_id: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphImportCounts {
    pub files: usize,
    pub classes: usize,
    pub functions: usize,
    pub imports: usize,
    pub relationships: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphSummary {
    pub files: i64,
    pub classes: i64,
    pub functions: i64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphStats {
    pub files: i64,
    pub classes: i64,
    pub functions: i64,
    pub memory_file_links: i64,
    pub memory_function_links: i64,
}

impl From<CodeGraphStats> for CodeGraphSummary {
    fn from(value: CodeGraphStats) -> Self {
        Self {
            files: value.files,
            classes: value.classes,
            functions: value.functions,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphContextPayload {
    pub memory_id: String,
    pub files: Vec<CodeGraphContextFile>,
    pub functions: Vec<CodeGraphContextFunction>,
    pub classes: Vec<CodeGraphContextClass>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphContextFile {
    #[serde(rename = "type")]
    pub kind: String,
    pub path: String,
    pub language: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphContextFunction {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub signature: String,
    pub docstring: String,
    pub complexity: i64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphContextClass {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub fully_qualified_name: String,
    pub docstring: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphNamedEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphSearchEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphEdgeEntry {
    pub caller: String,
    pub callee: String,
}

pub(crate) trait CodeGraphReaderBackend {
    fn stats(&self) -> Result<CodeGraphStats>;
    fn context_payload(&self, memory_id: &str) -> Result<CodeGraphContextPayload>;
    fn files(&self, pattern: Option<&str>, limit: u32) -> Result<Vec<String>>;
    fn functions(&self, file: Option<&str>, limit: u32) -> Result<Vec<CodeGraphNamedEntry>>;
    fn classes(&self, file: Option<&str>, limit: u32) -> Result<Vec<CodeGraphNamedEntry>>;
    fn search(&self, name: &str, limit: u32) -> Result<Vec<CodeGraphSearchEntry>>;
    fn callers(&self, name: &str, limit: u32) -> Result<Vec<CodeGraphEdgeEntry>>;
    fn callees(&self, name: &str, limit: u32) -> Result<Vec<CodeGraphEdgeEntry>>;
}

pub(crate) fn open_code_graph_reader(
    path_override: Option<&Path>,
) -> Result<Box<dyn CodeGraphReaderBackend>> {
    Ok(Box::new(KuzuCodeGraphReader::open(path_override)?))
}

struct KuzuCodeGraphReader {
    db: KuzuDatabase,
}

impl KuzuCodeGraphReader {
    fn open(path_override: Option<&Path>) -> Result<Self> {
        Ok(Self {
            db: open_kuzu_code_graph_db(path_override)?,
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&KuzuConnection<'_>) -> Result<T>) -> Result<T> {
        let conn = KuzuConnection::new(&self.db)?;
        ensure_memory_code_link_schema(&conn)?;
        f(&conn)
    }
}

impl CodeGraphReaderBackend for KuzuCodeGraphReader {
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

trait CodeGraphWriterBackend {
    fn import_blarify_output(&self, payload: &BlarifyOutput) -> Result<CodeGraphImportCounts>;
}

fn open_code_graph_writer(path_override: Option<&Path>) -> Result<Box<dyn CodeGraphWriterBackend>> {
    Ok(Box::new(KuzuCodeGraphWriter::open(path_override)?))
}

struct KuzuCodeGraphWriter {
    db: KuzuDatabase,
}

impl KuzuCodeGraphWriter {
    fn open(path_override: Option<&Path>) -> Result<Self> {
        Ok(Self {
            db: open_kuzu_code_graph_db(path_override)?,
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&KuzuConnection<'_>) -> Result<T>) -> Result<T> {
        let conn = KuzuConnection::new(&self.db)?;
        ensure_memory_code_link_schema(&conn)?;
        f(&conn)
    }
}

impl CodeGraphWriterBackend for KuzuCodeGraphWriter {
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

pub fn run_index_code(input: &Path, kuzu_path: Option<&Path>) -> Result<()> {
    let counts = import_blarify_json(input, kuzu_path)?;
    println!("{}", serde_json::to_string_pretty(&counts)?);
    Ok(())
}

pub fn import_scip_file(
    input_path: &Path,
    project_root: &Path,
    language_hint: Option<&str>,
    kuzu_path: Option<&Path>,
) -> Result<CodeGraphImportCounts> {
    if !input_path.exists() {
        bail!("SCIP index not found: {}", input_path.display());
    }
    let input_path = validate_index_path(input_path)?;
    let project_root = validate_index_path(project_root)?;

    let bytes = fs::read(&input_path)
        .with_context(|| format!("failed to read {}", input_path.display()))?;
    let index = ScipIndex::decode(bytes.as_slice())
        .with_context(|| format!("invalid SCIP protobuf in {}", input_path.display()))?;
    let payload = convert_scip_to_blarify(&index, &project_root, language_hint);

    let default_db_path;
    let path_override = match kuzu_path {
        Some(path) => Some(path),
        None => {
            default_db_path = default_code_graph_db_path_for_project(&project_root)?;
            Some(default_db_path.as_path())
        }
    };
    open_code_graph_writer(path_override)?.import_blarify_output(&payload)
}

pub fn import_blarify_json(
    input_path: &Path,
    kuzu_path: Option<&Path>,
) -> Result<CodeGraphImportCounts> {
    if !input_path.exists() {
        tracing::warn!(
            file = %input_path.file_name().unwrap_or_default().to_string_lossy(),
            "blarify JSON not found; skipping import"
        );
        return Ok(CodeGraphImportCounts::default());
    }
    let input_path = validate_index_path(input_path)?;
    validate_blarify_json_size(&input_path, BLARIFY_JSON_MAX_BYTES)?;

    let raw = fs::read_to_string(&input_path)
        .with_context(|| format!("failed to read {}", input_path.display()))?;
    let payload: BlarifyOutput = serde_json::from_str(&raw)
        .with_context(|| format!("invalid JSON in {}", input_path.display()))?;

    let inferred_db_path;
    let path_override = match kuzu_path {
        Some(path) => Some(path),
        None => {
            inferred_db_path = infer_code_graph_db_path_from_input(&input_path)?;
            Some(inferred_db_path.as_path())
        }
    };

    open_code_graph_writer(path_override)?.import_blarify_output(&payload)
}

pub(crate) fn open_kuzu_code_graph_db(path_override: Option<&Path>) -> Result<KuzuDatabase> {
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

fn default_code_graph_db_path() -> Result<PathBuf> {
    default_code_graph_db_path_for_project(
        &std::env::current_dir()
            .context("failed to resolve current directory for default code graph path")?,
    )
}

pub fn default_code_graph_db_path_for_project(project_root: &Path) -> Result<PathBuf> {
    Ok(project_root.join(".amplihack").join("kuzu_db"))
}

fn infer_code_graph_db_path_from_input(input_path: &Path) -> Result<PathBuf> {
    let Some(parent) = input_path.parent() else {
        return default_code_graph_db_path();
    };
    let is_blarify_json =
        input_path.file_name().and_then(|name| name.to_str()) == Some("blarify.json");
    let is_project_amplihack_dir =
        parent.file_name().and_then(|name| name.to_str()) == Some(".amplihack");
    if is_blarify_json && is_project_amplihack_dir {
        let Some(project_root) = parent.parent() else {
            return default_code_graph_db_path();
        };
        return default_code_graph_db_path_for_project(project_root);
    }
    default_code_graph_db_path()
}

pub(crate) fn init_kuzu_code_graph_schema(conn: &KuzuConnection<'_>) -> Result<()> {
    for statement in KUZU_CODE_GRAPH_SCHEMA {
        conn.query(statement)?;
    }
    Ok(())
}

pub(crate) fn ensure_memory_code_link_schema(conn: &KuzuConnection<'_>) -> Result<()> {
    init_kuzu_backend_schema(conn)?;
    init_kuzu_code_graph_schema(conn)?;
    for (memory_type, rel_table) in KUZU_MEMORY_FILE_LINK_TABLES {
        conn.query(&format!(
            "CREATE REL TABLE IF NOT EXISTS {rel_table}(FROM {memory_type} TO CodeFile, relevance_score DOUBLE, context STRING, timestamp TIMESTAMP)"
        ))?;
    }
    for (memory_type, rel_table) in KUZU_MEMORY_FUNCTION_LINK_TABLES {
        conn.query(&format!(
            "CREATE REL TABLE IF NOT EXISTS {rel_table}(FROM {memory_type} TO CodeFunction, relevance_score DOUBLE, context STRING, timestamp TIMESTAMP)"
        ))?;
    }
    Ok(())
}

fn normalize_match_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub(crate) fn code_memory_link_counts(conn: &KuzuConnection<'_>) -> Result<(i64, i64)> {
    ensure_memory_code_link_schema(conn)?;

    let file_links =
        KUZU_MEMORY_FILE_LINK_TABLES
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
        KUZU_MEMORY_FUNCTION_LINK_TABLES
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

fn link_memories_to_code_files_in_conn(conn: &KuzuConnection<'_>) -> Result<usize> {
    ensure_memory_code_link_schema(conn)?;

    let mut created = 0usize;
    let now = OffsetDateTime::now_utc();

    for (memory_type, rel_table) in KUZU_MEMORY_FILE_LINK_TABLES {
        let memories = kuzu_rows(
            conn,
            &format!(
                "MATCH (m:{memory_type}) WHERE m.metadata IS NOT NULL RETURN m.memory_id, m.metadata"
            ),
            vec![],
        )?;

        for row in memories {
            let memory_id = kuzu_string(row.first())?;
            let metadata_raw = match row.get(1) {
                Some(value) => kuzu_string(Some(value))?,
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

            let matching_files = kuzu_rows(
                conn,
                "MATCH (cf:CodeFile) WHERE cf.file_path CONTAINS $file_path OR $file_path CONTAINS cf.file_path RETURN cf.file_id",
                vec![("file_path", KuzuValue::String(file_path))],
            )?;

            for file_row in matching_files {
                let file_id = kuzu_string(file_row.first())?;
                let existing = kuzu_rows(
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
                    .map(|row| kuzu_i64(row.first()).unwrap_or(0))
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

    for (memory_type, rel_table) in KUZU_MEMORY_FUNCTION_LINK_TABLES {
        let memories = kuzu_rows(
            conn,
            &format!(
                "MATCH (m:{memory_type}) WHERE m.content IS NOT NULL RETURN m.memory_id, m.content"
            ),
            vec![],
        )?;

        for row in memories {
            let memory_id = kuzu_string(row.first())?;
            let content = match row.get(1) {
                Some(value) => kuzu_string(Some(value))?,
                None => continue,
            };
            if content.trim().is_empty() {
                continue;
            }

            let matching_functions = kuzu_rows(
                conn,
                "MATCH (f:CodeFunction) WHERE $content CONTAINS f.function_name RETURN f.function_id",
                vec![("content", KuzuValue::String(content))],
            )?;

            for function_row in matching_functions {
                let function_id = kuzu_string(function_row.first())?;
                let existing = kuzu_rows(
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
                    .map(|row| kuzu_i64(row.first()).unwrap_or(0))
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

pub fn summarize_code_graph(kuzu_path: Option<&Path>) -> Result<Option<CodeGraphSummary>> {
    let path = match kuzu_path {
        Some(path) => path.to_path_buf(),
        None => default_code_graph_db_path()?,
    };
    if !path.exists() {
        return Ok(None);
    }

    let stats = open_code_graph_reader(Some(&path))?.stats()?;
    Ok(Some(stats.into()))
}

fn scalar_count(conn: &KuzuConnection<'_>, query: &str) -> Result<i64> {
    let rows = kuzu_rows(conn, query, vec![])?;
    kuzu_i64(rows.first().and_then(|row| row.first()))
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

    let files = kuzu_rows(
        conn,
        &format!(
            "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[:{file_rel}]->(cf:CodeFile) RETURN cf.file_path, cf.language, cf.size_bytes ORDER BY cf.file_path"
        ),
        vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
    )?;
    let functions = kuzu_rows(
        conn,
        &format!(
            "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[:{function_rel}]->(f:CodeFunction) RETURN f.function_name, f.signature, f.docstring, f.cyclomatic_complexity ORDER BY f.function_name"
        ),
        vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
    )?;
    let classes = kuzu_rows(
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
                path: kuzu_string(row.first()).unwrap_or_default(),
                language: kuzu_string(row.get(1)).unwrap_or_default(),
                size_bytes: kuzu_i64(row.get(2)).unwrap_or_default(),
            })
            .collect(),
        functions: functions
            .iter()
            .map(|row| CodeGraphContextFunction {
                kind: "function".to_string(),
                name: kuzu_string(row.first()).unwrap_or_default(),
                signature: kuzu_string(row.get(1)).unwrap_or_default(),
                docstring: kuzu_string(row.get(2)).unwrap_or_default(),
                complexity: kuzu_i64(row.get(3)).unwrap_or_default(),
            })
            .collect(),
        classes: classes
            .iter()
            .map(|row| CodeGraphContextClass {
                kind: "class".to_string(),
                name: kuzu_string(row.first()).unwrap_or_default(),
                fully_qualified_name: kuzu_string(row.get(1)).unwrap_or_default(),
                docstring: kuzu_string(row.get(2)).unwrap_or_default(),
            })
            .collect(),
    })
}

fn resolve_memory_link_tables_in_conn(
    conn: &KuzuConnection<'_>,
    memory_id: &str,
) -> Result<Option<(&'static str, &'static str, &'static str)>> {
    for ((memory_type, file_rel), (paired_type, function_rel)) in KUZU_MEMORY_FILE_LINK_TABLES
        .iter()
        .zip(KUZU_MEMORY_FUNCTION_LINK_TABLES.iter())
    {
        debug_assert_eq!(memory_type, paired_type);
        let rows = kuzu_rows(
            conn,
            &format!("MATCH (m:{memory_type} {{memory_id: $memory_id}}) RETURN COUNT(m)"),
            vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
        )?;
        if kuzu_i64(rows.first().and_then(|row| row.first()))? > 0 {
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
        kuzu_rows(
            conn,
            "MATCH (cf:CodeFile) WHERE cf.file_path CONTAINS $pattern RETURN cf.file_path ORDER BY cf.file_path LIMIT $lim",
            vec![
                ("pattern", KuzuValue::String(pattern.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        kuzu_rows(
            conn,
            "MATCH (cf:CodeFile) RETURN cf.file_path ORDER BY cf.file_path LIMIT $lim",
            vec![("lim", KuzuValue::UInt64(u64::from(limit)))],
        )?
    };

    Ok(rows
        .iter()
        .map(|row| kuzu_string(row.first()).unwrap_or_default())
        .collect())
}

fn list_code_functions_in_conn(
    conn: &KuzuConnection<'_>,
    file: Option<&str>,
    limit: u32,
) -> Result<Vec<CodeGraphNamedEntry>> {
    let rows = if let Some(file) = file {
        kuzu_rows(
            conn,
            "MATCH (f:CodeFunction)-[:DEFINED_IN]->(cf:CodeFile) WHERE cf.file_path CONTAINS $file AND NOT f.function_name CONTAINS '().(' RETURN f.function_name, cf.file_path ORDER BY cf.file_path, f.function_name LIMIT $lim",
            vec![
                ("file", KuzuValue::String(file.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        kuzu_rows(
            conn,
            "MATCH (f:CodeFunction) WHERE NOT f.function_name CONTAINS '().(' RETURN f.function_name ORDER BY f.function_name LIMIT $lim",
            vec![("lim", KuzuValue::UInt64(u64::from(limit)))],
        )?
    };

    Ok(rows
        .iter()
        .map(|row| CodeGraphNamedEntry {
            name: kuzu_string(row.first()).unwrap_or_default(),
            file: row
                .get(1)
                .map(|value| kuzu_string(Some(value)).unwrap_or_default()),
        })
        .collect())
}

fn list_code_classes_in_conn(
    conn: &KuzuConnection<'_>,
    file: Option<&str>,
    limit: u32,
) -> Result<Vec<CodeGraphNamedEntry>> {
    let rows = if let Some(file) = file {
        kuzu_rows(
            conn,
            "MATCH (c:CodeClass)-[:CLASS_DEFINED_IN]->(cf:CodeFile) WHERE cf.file_path CONTAINS $file RETURN c.class_name, cf.file_path ORDER BY cf.file_path, c.class_name LIMIT $lim",
            vec![
                ("file", KuzuValue::String(file.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        kuzu_rows(
            conn,
            "MATCH (c:CodeClass) RETURN c.class_name ORDER BY c.class_name LIMIT $lim",
            vec![("lim", KuzuValue::UInt64(u64::from(limit)))],
        )?
    };

    Ok(rows
        .iter()
        .map(|row| CodeGraphNamedEntry {
            name: kuzu_string(row.first()).unwrap_or_default(),
            file: row
                .get(1)
                .map(|value| kuzu_string(Some(value)).unwrap_or_default()),
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
        let rows = kuzu_rows(
            conn,
            query,
            vec![
                ("name", KuzuValue::String(name.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?;
        payload.extend(rows.into_iter().map(|row| CodeGraphSearchEntry {
            kind: kind.to_string(),
            name: kuzu_string(row.first()).unwrap_or_default(),
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
    let rows = kuzu_rows(
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
            caller: kuzu_string(row.first()).unwrap_or_default(),
            callee: kuzu_string(row.get(1)).unwrap_or_default(),
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
            kuzu_rows(
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
            kuzu_rows(
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
            kuzu_rows(
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
            kuzu_rows(
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
            kuzu_rows(
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
            kuzu_rows(
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
            kuzu_rows(
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
            kuzu_rows(
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
            kuzu_rows(
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

        kuzu_rows(
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

    kuzu_rows(
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

    kuzu_rows(
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

    kuzu_rows(
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
    let rows = kuzu_rows(conn, query, params)?;
    Ok(rows
        .first()
        .map(|row| kuzu_i64(row.first()).unwrap_or(0))
        .unwrap_or(0)
        > 0)
}

fn parse_blarify_timestamp(value: Option<&str>) -> Option<OffsetDateTime> {
    let parsed = DateTime::parse_from_rfc3339(value?).ok()?;
    OffsetDateTime::from_unix_timestamp(parsed.timestamp()).ok()
}

fn convert_scip_to_blarify(
    index: &ScipIndex,
    project_root: &Path,
    language_hint: Option<&str>,
) -> BlarifyOutput {
    let mut payload = BlarifyOutput::default();

    for doc in &index.documents {
        let language = if doc.language.trim().is_empty() {
            language_hint.unwrap_or_default().to_string()
        } else {
            doc.language.clone()
        };
        let file_path = project_root.join(&doc.relative_path);
        let file_path = file_path.to_string_lossy().replace('\\', "/");
        let lines_of_code = doc.text.lines().count() as i64;
        payload.files.push(BlarifyFile {
            path: file_path.clone(),
            language,
            lines_of_code,
            last_modified: None,
        });

        for symbol in &doc.symbols {
            let symbol_name = symbol.symbol.trim();
            if symbol_name.is_empty() {
                continue;
            }

            let line_number = find_definition_line(symbol_name, &doc.occurrences);
            let docstring = symbol.documentation.join(" ");

            if is_function_symbol(symbol) {
                payload.functions.push(BlarifyFunction {
                    id: symbol_name.to_string(),
                    name: extract_name_from_symbol(symbol_name),
                    file_path: file_path.clone(),
                    line_number,
                    docstring,
                    parameters: Vec::new(),
                    return_type: String::new(),
                    is_async: false,
                    complexity: 0,
                    class_id: enclosing_class_id(symbol),
                });
            } else if is_class_symbol(symbol) {
                payload.classes.push(BlarifyClass {
                    id: symbol_name.to_string(),
                    name: extract_name_from_symbol(symbol_name),
                    file_path: file_path.clone(),
                    line_number,
                    docstring,
                    is_abstract: matches!(symbol.kind, SCIP_KIND_INTERFACE | SCIP_KIND_TRAIT),
                });
            }
        }
    }

    payload
}

fn find_definition_line(symbol: &str, occurrences: &[ScipOccurrence]) -> i64 {
    occurrences
        .iter()
        .find(|occ| occ.symbol == symbol && (occ.symbol_roles & SCIP_SYMBOL_ROLE_DEFINITION) != 0)
        .and_then(|occ| occ.range.first().copied())
        .map(i64::from)
        .unwrap_or(0)
}

fn extract_name_from_symbol(symbol: &str) -> String {
    if let Some(part) = symbol.rsplit('/').next() {
        return part
            .trim_end_matches('.')
            .trim_end_matches("()")
            .to_string();
    }
    if let Some(part) = symbol.split_whitespace().last() {
        return part
            .trim_end_matches('.')
            .trim_end_matches("()")
            .to_string();
    }
    symbol
        .trim_end_matches('.')
        .trim_end_matches("()")
        .to_string()
}

fn enclosing_class_id(symbol: &ScipSymbolInformation) -> Option<String> {
    let enclosing = symbol.enclosing_symbol.trim();
    if enclosing.is_empty() || !is_class_symbol_by_name(enclosing) {
        return None;
    }
    Some(enclosing.to_string())
}

fn is_function_symbol(symbol: &ScipSymbolInformation) -> bool {
    matches!(
        symbol.kind,
        SCIP_KIND_FUNCTION
            | SCIP_KIND_METHOD
            | SCIP_KIND_CONSTRUCTOR
            | SCIP_KIND_PROTOCOL_METHOD
            | SCIP_KIND_STATIC_METHOD
            | SCIP_KIND_TRAIT_METHOD
            | SCIP_KIND_ABSTRACT_METHOD
            | SCIP_KIND_PURE_VIRTUAL_METHOD
    ) || symbol.symbol.contains('(')
}

fn is_class_symbol(symbol: &ScipSymbolInformation) -> bool {
    matches!(
        symbol.kind,
        SCIP_KIND_CLASS
            | SCIP_KIND_INTERFACE
            | SCIP_KIND_STRUCT
            | SCIP_KIND_TRAIT
            | SCIP_KIND_OBJECT
            | SCIP_KIND_TYPE
            | SCIP_KIND_MODULE
            | SCIP_KIND_ENUM
    ) || is_class_symbol_by_name(&symbol.symbol)
}

fn is_class_symbol_by_name(symbol: &str) -> bool {
    if symbol.contains('(') {
        return false;
    }
    let name = extract_name_from_symbol(symbol);
    !name.is_empty()
        && name.chars().next().is_some_and(|ch| ch.is_uppercase())
        && !name.chars().all(|ch| ch.is_uppercase())
}

const SCIP_SYMBOL_ROLE_DEFINITION: i32 = 1;
const SCIP_KIND_CLASS: i32 = 7;
const SCIP_KIND_CONSTRUCTOR: i32 = 9;
const SCIP_KIND_ENUM: i32 = 11;
const SCIP_KIND_FUNCTION: i32 = 17;
const SCIP_KIND_METHOD: i32 = 26;
const SCIP_KIND_INTERFACE: i32 = 21;
const SCIP_KIND_MODULE: i32 = 29;
const SCIP_KIND_OBJECT: i32 = 33;
const SCIP_KIND_PROTOCOL_METHOD: i32 = 68;
const SCIP_KIND_PURE_VIRTUAL_METHOD: i32 = 69;
const SCIP_KIND_STATIC_METHOD: i32 = 80;
const SCIP_KIND_STRUCT: i32 = 49;
const SCIP_KIND_TRAIT: i32 = 53;
const SCIP_KIND_TRAIT_METHOD: i32 = 70;
const SCIP_KIND_TYPE: i32 = 54;
const SCIP_KIND_ABSTRACT_METHOD: i32 = 66;

#[derive(Clone, PartialEq, Message)]
struct ScipIndex {
    #[prost(message, repeated, tag = "2")]
    documents: Vec<ScipDocument>,
}

#[derive(Clone, PartialEq, Message)]
struct ScipDocument {
    #[prost(string, tag = "4")]
    language: String,
    #[prost(string, tag = "1")]
    relative_path: String,
    #[prost(message, repeated, tag = "2")]
    occurrences: Vec<ScipOccurrence>,
    #[prost(message, repeated, tag = "3")]
    symbols: Vec<ScipSymbolInformation>,
    #[prost(string, tag = "5")]
    text: String,
}

#[derive(Clone, PartialEq, Message)]
struct ScipSymbolInformation {
    #[prost(string, tag = "1")]
    symbol: String,
    #[prost(string, repeated, tag = "3")]
    documentation: Vec<String>,
    #[prost(int32, tag = "5")]
    kind: i32,
    #[prost(string, tag = "6")]
    display_name: String,
    #[prost(string, tag = "8")]
    enclosing_symbol: String,
}

#[derive(Clone, PartialEq, Message)]
struct ScipOccurrence {
    #[prost(int32, repeated, tag = "1")]
    range: Vec<i32>,
    #[prost(string, tag = "2")]
    symbol: String,
    #[prost(int32, tag = "3")]
    symbol_roles: i32,
}

// ── Issue #77 security & validation functions ─────────────────────────────
//
// These functions implement the security and validation contracts for Issue #77.
// All implementations are complete and the tests below pass.

/// Validate that `path` is safe to use as a project root or input path.
///
/// Contract:
/// - Canonicalize the path (resolve symlinks / `..` components).
/// - Return `Err` if the resolved path starts with `/proc`, `/sys`, or `/dev`.
/// - Return `Ok(canonical_path)` for all other paths.
///
/// Security note (P2-PATH): callers must use the *returned* canonical path,
/// not the original input, to prevent TOCTOU races.
pub(crate) fn validate_index_path(path: &Path) -> Result<PathBuf> {
    for blocked in [Path::new("/proc"), Path::new("/sys"), Path::new("/dev")] {
        if path.starts_with(blocked) {
            bail!("blocked unsafe path prefix: {}", blocked.display());
        }
    }

    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", path.display()))?;
    for blocked in [Path::new("/proc"), Path::new("/sys"), Path::new("/dev")] {
        if canonical.starts_with(blocked) {
            bail!("blocked unsafe path prefix: {}", blocked.display());
        }
    }
    Ok(canonical)
}

/// Assert that the Kuzu DB directory has restrictive Unix permissions.
///
/// Contract (P1-PERM, Unix only):
/// - The DB *parent* directory must be mode `0o700`.
/// - If Kuzu created a DB *file* (not a directory), that file must be `0o600`.
/// - On non-Unix platforms this is a no-op (returns `Ok(())`).
///
/// Must be called *after* `open_kuzu_code_graph_db()` has initialised the DB
/// so the path exists on disk.
#[cfg_attr(not(unix), allow(unused_variables))]
pub(crate) fn enforce_db_permissions(db_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        if let Some(parent) = db_path.parent()
            && parent.exists()
        {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("failed to secure {}", parent.display()))?;
        }

        if db_path.exists() {
            let mode = if db_path.is_dir() { 0o700 } else { 0o600 };
            fs::set_permissions(db_path, fs::Permissions::from_mode(mode))
                .with_context(|| format!("failed to secure {}", db_path.display()))?;
        }
    }
    Ok(())
}

/// Guard against deserialising a pathologically large `blarify.json`.
///
/// Contract (P2-SIZE):
/// - If the file at `path` is larger than `max_bytes`, return `Err` with a
///   message containing "size" or "large".
/// - If the file does not exist, return `Err` (caller decides how to handle).
/// - If the file is within the limit, return `Ok(())`.
///
/// The production limit is 500 MiB (`500 * 1024 * 1024`).  Tests may pass a
/// smaller limit to exercise the guard without writing 500 MB of data.
pub(crate) fn validate_blarify_json_size(path: &Path, max_bytes: u64) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.len() > max_bytes {
        bail!(
            "blarify JSON size {} exceeds configured limit {} bytes",
            metadata.len(),
            max_bytes
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{cwd_env_lock, restore_cwd, set_cwd};
    use kuzu::Connection as KuzuConnection;
    use tempfile::TempDir;

    fn sample_blarify_output() -> BlarifyOutput {
        serde_json::from_value(serde_json::json!({
            "files": [
                {
                    "path": "src/example/module.py",
                    "language": "python",
                    "lines_of_code": 100,
                    "last_modified": "2025-01-01T00:00:00Z"
                },
                {
                    "path": "src/example/utils.py",
                    "language": "python",
                    "lines_of_code": 50,
                    "last_modified": "2025-01-01T00:00:00Z"
                }
            ],
            "classes": [{
                "id": "class:Example",
                "name": "Example",
                "file_path": "src/example/module.py",
                "line_number": 10,
                "docstring": "Example class for testing.",
                "is_abstract": false
            }],
            "functions": [
                {
                    "id": "func:Example.process",
                    "name": "process",
                    "file_path": "src/example/module.py",
                    "line_number": 20,
                    "docstring": "Process data.",
                    "parameters": ["self", "data"],
                    "return_type": "str",
                    "is_async": false,
                    "complexity": 3,
                    "class_id": "class:Example"
                },
                {
                    "id": "func:helper",
                    "name": "helper",
                    "file_path": "src/example/utils.py",
                    "line_number": 5,
                    "docstring": "Helper function.",
                    "parameters": ["x"],
                    "return_type": "int",
                    "is_async": false,
                    "complexity": 1,
                    "class_id": null
                }
            ],
            "imports": [{
                "source_file": "src/example/module.py",
                "target_file": "src/example/utils.py",
                "symbol": "helper",
                "alias": null
            }],
            "relationships": [{
                "type": "CALLS",
                "source_id": "func:Example.process",
                "target_id": "func:helper"
            }]
        }))
        .unwrap()
    }

    fn temp_code_graph_db() -> Result<(TempDir, PathBuf)> {
        let dir = TempDir::new().map_err(|e| anyhow::anyhow!("tempdir: {e}"))?;
        let db_path = dir.path().join("code-graph.kuzu");
        Ok((dir, db_path))
    }

    #[test]
    fn import_blarify_json_populates_kuzu_code_graph() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        let json_dir = TempDir::new().unwrap();
        let json_path = json_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
        )
        .unwrap();

        let counts = import_blarify_json(&json_path, Some(&db_path)).unwrap();

        assert_eq!(
            counts,
            CodeGraphImportCounts {
                files: 2,
                classes: 1,
                functions: 2,
                imports: 1,
                relationships: 1,
            }
        );

        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();
        let rows = kuzu_rows(&conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)", vec![]).unwrap();
        assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 2);
        let rows = kuzu_rows(
            &conn,
            "MATCH (source:CodeFunction {function_id: $source_id})-[r:CALLS]->(target:CodeFunction {function_id: $target_id}) RETURN COUNT(r)",
            vec![
                ("source_id", KuzuValue::String("func:Example.process".to_string())),
                ("target_id", KuzuValue::String("func:helper".to_string())),
            ],
        )
        .unwrap();
        assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
    }

    #[test]
    fn import_blarify_json_updates_without_duplicates() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        let json_dir = TempDir::new().unwrap();
        let json_path = json_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
        )
        .unwrap();

        let first = import_blarify_json(&json_path, Some(&db_path)).unwrap();
        let second = import_blarify_json(&json_path, Some(&db_path)).unwrap();

        assert_eq!(first.files, 2);
        assert_eq!(second.files, 2);

        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();
        let rows = kuzu_rows(&conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)", vec![]).unwrap();
        assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 2);
    }

    #[test]
    fn import_blarify_json_links_semantic_memory_by_metadata_file() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();
        init_kuzu_backend_schema(&conn).unwrap();
        let now = OffsetDateTime::now_utc();

        let mut create_memory = conn
            .prepare(
                "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, category: $category, confidence_score: $confidence_score, last_updated: $last_updated, version: $version, title: $title, metadata: $metadata, tags: $tags, created_at: $created_at, accessed_at: $accessed_at, agent_id: $agent_id})",
            )
            .unwrap();
        conn.execute(
            &mut create_memory,
            vec![
                ("memory_id", KuzuValue::String("mem-1".to_string())),
                ("concept", KuzuValue::String("Example memory".to_string())),
                (
                    "content",
                    KuzuValue::String("Remember module.py".to_string()),
                ),
                ("category", KuzuValue::String("session_end".to_string())),
                ("confidence_score", KuzuValue::Double(1.0)),
                ("last_updated", KuzuValue::Timestamp(now)),
                ("version", KuzuValue::Int64(1)),
                ("title", KuzuValue::String("Example".to_string())),
                (
                    "metadata",
                    KuzuValue::String(r#"{"file":"src/example/module.py"}"#.to_string()),
                ),
                ("tags", KuzuValue::String(r#"["learning"]"#.to_string())),
                ("created_at", KuzuValue::Timestamp(now)),
                ("accessed_at", KuzuValue::Timestamp(now)),
                ("agent_id", KuzuValue::String("agent-1".to_string())),
            ],
        )
        .unwrap();

        let json_dir = TempDir::new().unwrap();
        let json_path = json_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
        )
        .unwrap();

        import_blarify_json(&json_path, Some(&db_path)).unwrap();
        import_blarify_json(&json_path, Some(&db_path)).unwrap();

        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();
        let rows = kuzu_rows(
            &conn,
            "MATCH (m:SemanticMemory {memory_id: $memory_id})-[r:RELATES_TO_FILE_SEMANTIC]->(cf:CodeFile {file_id: $file_id}) RETURN COUNT(r)",
            vec![
                ("memory_id", KuzuValue::String("mem-1".to_string())),
                (
                    "file_id",
                    KuzuValue::String("src/example/module.py".to_string()),
                ),
            ],
        )
        .unwrap();
        assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
    }

    #[test]
    fn import_blarify_json_links_semantic_memory_by_function_name() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();
        init_kuzu_backend_schema(&conn).unwrap();
        let now = OffsetDateTime::now_utc();

        let mut create_memory = conn
            .prepare(
                "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, category: $category, confidence_score: $confidence_score, last_updated: $last_updated, version: $version, title: $title, metadata: $metadata, tags: $tags, created_at: $created_at, accessed_at: $accessed_at, agent_id: $agent_id})",
            )
            .unwrap();
        conn.execute(
            &mut create_memory,
            vec![
                ("memory_id", KuzuValue::String("mem-func".to_string())),
                ("concept", KuzuValue::String("Helper memory".to_string())),
                (
                    "content",
                    KuzuValue::String("Remember to call helper before returning.".to_string()),
                ),
                ("category", KuzuValue::String("session_end".to_string())),
                ("confidence_score", KuzuValue::Double(1.0)),
                ("last_updated", KuzuValue::Timestamp(now)),
                ("version", KuzuValue::Int64(1)),
                ("title", KuzuValue::String("Helper".to_string())),
                ("metadata", KuzuValue::String("{}".to_string())),
                ("tags", KuzuValue::String(r#"["learning"]"#.to_string())),
                ("created_at", KuzuValue::Timestamp(now)),
                ("accessed_at", KuzuValue::Timestamp(now)),
                ("agent_id", KuzuValue::String("agent-1".to_string())),
            ],
        )
        .unwrap();

        let json_dir = TempDir::new().unwrap();
        let json_path = json_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
        )
        .unwrap();

        import_blarify_json(&json_path, Some(&db_path)).unwrap();
        import_blarify_json(&json_path, Some(&db_path)).unwrap();

        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();
        let rows = kuzu_rows(
            &conn,
            "MATCH (m:SemanticMemory {memory_id: $memory_id})-[r:RELATES_TO_FUNCTION_SEMANTIC]->(f:CodeFunction {function_id: $function_id}) RETURN COUNT(r)",
            vec![
                ("memory_id", KuzuValue::String("mem-func".to_string())),
                ("function_id", KuzuValue::String("func:helper".to_string())),
            ],
        )
        .unwrap();
        assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
    }

    #[test]
    fn default_code_graph_db_path_uses_project_local_store() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = TempDir::new().unwrap();
        let previous = set_cwd(dir.path()).unwrap();

        let path = default_code_graph_db_path().unwrap();

        restore_cwd(&previous).unwrap();
        assert_eq!(path, dir.path().join(".amplihack").join("kuzu_db"));
    }

    #[test]
    fn summarize_code_graph_reads_imported_counts() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        let json_dir = TempDir::new().unwrap();
        let json_path = json_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
        )
        .unwrap();

        import_blarify_json(&json_path, Some(&db_path)).unwrap();
        let summary = summarize_code_graph(Some(&db_path))
            .unwrap()
            .expect("summary should exist");

        assert_eq!(
            summary,
            CodeGraphSummary {
                files: 2,
                classes: 1,
                functions: 2,
            }
        );
    }

    fn sample_scip_index() -> ScipIndex {
        ScipIndex {
            documents: vec![ScipDocument {
                language: "python".to_string(),
                relative_path: "src/example/module.py".to_string(),
                text: "class Example:\n    pass\n\ndef helper():\n    return 1\n".to_string(),
                occurrences: vec![
                    ScipOccurrence {
                        range: vec![0, 6, 0, 13],
                        symbol: "scip-python python pkg src/example/module.py/Example.".to_string(),
                        symbol_roles: SCIP_SYMBOL_ROLE_DEFINITION,
                    },
                    ScipOccurrence {
                        range: vec![3, 4, 3, 10],
                        symbol: "scip-python python pkg src/example/module.py/helper()."
                            .to_string(),
                        symbol_roles: SCIP_SYMBOL_ROLE_DEFINITION,
                    },
                ],
                symbols: vec![
                    ScipSymbolInformation {
                        symbol: "scip-python python pkg src/example/module.py/Example.".to_string(),
                        documentation: vec!["Example class".to_string()],
                        kind: SCIP_KIND_CLASS,
                        display_name: "Example".to_string(),
                        enclosing_symbol: String::new(),
                    },
                    ScipSymbolInformation {
                        symbol: "scip-python python pkg src/example/module.py/helper()."
                            .to_string(),
                        documentation: vec!["Helper".to_string()],
                        kind: SCIP_KIND_FUNCTION,
                        display_name: "helper".to_string(),
                        enclosing_symbol: String::new(),
                    },
                ],
            }],
        }
    }

    #[test]
    fn import_scip_file_populates_kuzu_code_graph() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        let project_dir = TempDir::new().unwrap();
        let src_dir = project_dir.path().join("src/example");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("module.py"), "class Example:\n    pass\n").unwrap();

        let scip_dir = TempDir::new().unwrap();
        let scip_path = scip_dir.path().join("index.scip");
        fs::write(&scip_path, sample_scip_index().encode_to_vec()).unwrap();

        let counts = import_scip_file(
            &scip_path,
            project_dir.path(),
            Some("python"),
            Some(&db_path),
        )
        .unwrap();

        assert_eq!(counts.files, 1);
        assert_eq!(counts.classes, 1);
        assert_eq!(counts.functions, 1);

        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();
        let rows = kuzu_rows(&conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)", vec![]).unwrap();
        assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
        let rows = kuzu_rows(&conn, "MATCH (f:CodeFunction) RETURN COUNT(f)", vec![]).unwrap();
        assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
        let rows = kuzu_rows(&conn, "MATCH (c:CodeClass) RETURN COUNT(c)", vec![]).unwrap();
        assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
    }

    // ── Issue #77 security & validation tests ─────────────────────────────
    //
    // These tests verify the security and validation behaviour implemented for
    // Issue #77.  All four groups pass with the current implementation:
    //
    //   1. `import_blarify_json_absent_returns_ok_with_empty_counts` — PASSES:
    //      absent blarify.json returns Ok(empty counts) with a tracing::warn!.
    //
    //   2. `enforce_db_permissions_sets_restrictive_unix_modes` — PASSES:
    //      `enforce_db_permissions()` sets 0o700/0o600 on DB paths (Unix).
    //
    //   3. `validate_index_path_*` — PASS: path canonicalization + blocklist
    //      for /proc, /sys, /dev is implemented and working.
    //
    //   4. `validate_blarify_json_size_*` — PASS: size guard rejects files
    //      exceeding BLARIFY_JSON_MAX_BYTES before deserialization.

    // ── (1) Blarify fallback: absent file → Ok(empty) not Err ─────────────

    /// AC7 / R5: When blarify.json does not exist the live path must degrade
    /// gracefully (log a warning, return empty counts) rather than aborting
    /// with an error.  Silent failure is prohibited — tracing::warn! fires.
    ///
    /// AC7 blarify fallback: absent blarify.json returns Ok(empty counts)
    /// with a tracing::warn! emitted (graceful degradation, not hard failure).
    #[test]
    fn import_blarify_json_absent_returns_ok_with_empty_counts() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        // Use a path that is guaranteed not to exist.
        let missing = PathBuf::from("/tmp/__amplihack_tdd_absent_blarify_i77__.json");
        let _ = std::fs::remove_file(&missing); // ensure it really is absent

        let result = import_blarify_json(&missing, Some(&db_path));

        assert!(
            result.is_ok(),
            "Expected Ok(empty counts) when blarify.json is absent \
             (graceful fallback), but got Err: {:?}",
            result.err()
        );
        let counts = result.unwrap();
        assert_eq!(
            counts,
            CodeGraphImportCounts {
                files: 0,
                classes: 0,
                functions: 0,
                imports: 0,
                relationships: 0,
            },
            "Expected all-zero counts for absent blarify.json"
        );
    }

    // ── (2) DB permissions enforcement ────────────────────────────────────

    /// P1-PERM: After Kuzu initialises the database the parent directory must
    /// be mode 0o700 and the DB path itself 0o600 (or 0o700 if Kuzu creates a
    /// directory rather than a flat file).
    ///
    /// P1-PERM: DB parent directory must be 0o700; DB file/dir must be 0o600/0o700.
    #[test]
    #[cfg(unix)]
    fn enforce_db_permissions_sets_restrictive_unix_modes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("secured.kuzu");

        // Initialise the DB so the path exists on disk.
        let _db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        drop(_db);

        // Call the enforcement function under test.
        enforce_db_permissions(&db_path).expect("enforce_db_permissions should succeed");

        // The parent directory must be 0o700.
        let parent_meta = fs::metadata(dir.path()).unwrap();
        let parent_mode = parent_meta.permissions().mode() & 0o777;
        assert_eq!(
            parent_mode, 0o700,
            "parent directory should be mode 0o700, got 0o{parent_mode:o}"
        );

        // The DB itself (file or directory Kuzu creates) must be 0o600 / 0o700.
        if db_path.exists() {
            let db_meta = fs::metadata(&db_path).unwrap();
            let db_mode = db_meta.permissions().mode() & 0o777;
            assert!(
                db_mode == 0o600 || db_mode == 0o700,
                "DB path should be mode 0o600 or 0o700, got 0o{db_mode:o}"
            );
        }
    }

    // ── (3) Path validation ───────────────────────────────────────────────

    /// P2-PATH: `/proc` subtrees must be rejected.
    #[test]
    fn validate_index_path_blocks_proc_prefix() {
        let result = validate_index_path(Path::new("/proc/1/mem"));
        assert!(
            result.is_err(),
            "Expected Err for /proc path, got Ok({:?})",
            result.ok()
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("/proc") || msg.to_lowercase().contains("blocked"),
            "Error message should mention the blocked prefix, got: {msg}"
        );
    }

    /// P2-PATH: `/sys` subtrees must be rejected.
    #[test]
    fn validate_index_path_blocks_sys_prefix() {
        let result = validate_index_path(Path::new("/sys/kernel/config"));
        assert!(
            result.is_err(),
            "Expected Err for /sys path, got Ok({:?})",
            result.ok()
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("/sys") || msg.to_lowercase().contains("blocked"),
            "Error message should mention the blocked prefix, got: {msg}"
        );
    }

    /// P2-PATH: `/dev` subtrees must be rejected.
    #[test]
    fn validate_index_path_blocks_dev_prefix() {
        let result = validate_index_path(Path::new("/dev/null"));
        assert!(
            result.is_err(),
            "Expected Err for /dev path, got Ok({:?})",
            result.ok()
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("/dev") || msg.to_lowercase().contains("blocked"),
            "Error message should mention the blocked prefix, got: {msg}"
        );
    }

    /// P2-PATH: Normal temp paths must be allowed through.
    #[test]
    fn validate_index_path_allows_normal_temp_path() {
        let dir = TempDir::new().unwrap();
        // Create a real subdirectory so canonicalize() can resolve it.
        let project_dir = dir.path().join("my_project");
        fs::create_dir_all(&project_dir).unwrap();

        let result = validate_index_path(&project_dir);
        assert!(
            result.is_ok(),
            "Expected Ok for a normal temp directory, got Err: {:?}",
            result.err()
        );
        // The returned path must be the canonicalized form.
        let canonical = result.unwrap();
        assert!(
            canonical.is_absolute(),
            "validate_index_path must return an absolute canonical path"
        );
    }

    /// P2-PATH: Paths that *look* like blocked prefixes but are not (e.g.
    /// `/proc_data`) must be allowed.
    #[test]
    fn validate_index_path_allows_path_with_proc_in_name_not_prefix() {
        let dir = TempDir::new().unwrap();
        // e.g. /tmp/abc/proc_data — should NOT be blocked
        let allowed = dir.path().join("proc_data");
        fs::create_dir_all(&allowed).unwrap();

        let result = validate_index_path(&allowed);
        assert!(
            result.is_ok(),
            "Path containing 'proc' as a *directory name* (not prefix) should be \
             allowed, got Err: {:?}",
            result.err()
        );
    }

    // ── (4) Blarify JSON size guard ───────────────────────────────────────

    /// P2-SIZE: A file that exceeds the configured byte limit must be rejected
    /// BEFORE serde_json deserialization to prevent memory exhaustion.
    #[test]
    fn validate_blarify_json_size_rejects_file_exceeding_limit() {
        let dir = TempDir::new().unwrap();
        let json_path = dir.path().join("blarify.json");
        // Write 100 bytes of valid-ish JSON-like content.
        fs::write(
            &json_path,
            b"{\"files\":[],\"classes\":[],\"functions\":[],\"imports\":[],\"relationships\":[]}",
        )
        .unwrap();

        // With a 0-byte limit, ANY non-empty file must be rejected.
        let result = validate_blarify_json_size(&json_path, 0);
        assert!(
            result.is_err(),
            "Expected Err when file exceeds the 0-byte limit, got Ok"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.to_lowercase().contains("size")
                || msg.to_lowercase().contains("large")
                || msg.to_lowercase().contains("exceed")
                || msg.to_lowercase().contains("limit"),
            "Error message should explain why the file was rejected, got: {msg}"
        );
    }

    /// P2-SIZE: A file that is WITHIN the configured limit must be accepted.
    #[test]
    fn validate_blarify_json_size_accepts_file_within_limit() {
        let dir = TempDir::new().unwrap();
        let json_path = dir.path().join("blarify.json");
        let content =
            b"{\"files\":[],\"classes\":[],\"functions\":[],\"imports\":[],\"relationships\":[]}";
        fs::write(&json_path, content).unwrap();

        // 500 MiB limit — content is ~80 bytes, well within bounds.
        let max: u64 = 500 * 1024 * 1024;
        let result = validate_blarify_json_size(&json_path, max);
        assert!(
            result.is_ok(),
            "Expected Ok when file is within the size limit, got Err: {:?}",
            result.err()
        );
    }

    /// P2-SIZE: A missing file must also be rejected (not silently pass the
    /// size guard to then crash in the reader).
    #[test]
    fn validate_blarify_json_size_rejects_missing_file() {
        let missing = PathBuf::from("/tmp/__amplihack_tdd_absent_size_check_i77__.json");
        let _ = std::fs::remove_file(&missing);

        let result = validate_blarify_json_size(&missing, 500 * 1024 * 1024);
        assert!(
            result.is_err(),
            "Expected Err for a missing file in validate_blarify_json_size, got Ok"
        );
    }
}
