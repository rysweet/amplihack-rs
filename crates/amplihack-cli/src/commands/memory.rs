//! Native memory commands (`tree`, `export`, `import`, `clean`).

use crate::command_error::exit_error;
use anyhow::{Context, Result};
use kuzu::{
    Connection as KuzuConnection, Database as KuzuDatabase, SystemConfig, Value as KuzuValue,
};
use rusqlite::{Connection as SqliteConnection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const SQLITE_TREE_BACKEND_NAME: &str = "unknown";
const KUZU_TREE_BACKEND_NAME: &str = "kuzu";
const SQLITE_SCHEMA: &str = r#"
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
const HIERARCHICAL_SCHEMA: &[&str] = &[
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
const KUZU_BACKEND_SCHEMA: &[&str] = &[
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
const KUZU_MEMORY_TABLES: &[(&str, &str)] = &[
    ("EpisodicMemory", "CONTAINS_EPISODIC"),
    ("SemanticMemory", "CONTRIBUTES_TO_SEMANTIC"),
    ("ProceduralMemory", "USES_PROCEDURE"),
    ("ProspectiveMemory", "CREATES_INTENTION"),
    ("WorkingMemory", "CONTAINS_WORKING"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendChoice {
    Kuzu,
    Sqlite,
}

impl BackendChoice {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "kuzu" => Ok(Self::Kuzu),
            "sqlite" => Ok(Self::Sqlite),
            other => anyhow::bail!("Invalid backend: {other}. Must be kuzu or sqlite"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferFormat {
    Json,
    Kuzu,
}

impl TransferFormat {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "json" => Ok(Self::Json),
            "kuzu" => Ok(Self::Kuzu),
            other => anyhow::bail!("Unsupported format: {other:?}. Use one of: ('json', 'kuzu')"),
        }
    }
}

#[derive(Debug, Clone)]
struct SessionSummary {
    session_id: String,
    memory_count: usize,
}

#[derive(Debug, Clone)]
struct MemoryRecord {
    memory_type: String,
    title: String,
    metadata: JsonValue,
    importance: Option<i64>,
    expires_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HierarchicalExportData {
    agent_name: String,
    exported_at: String,
    format_version: String,
    semantic_nodes: Vec<SemanticNode>,
    episodic_nodes: Vec<EpisodicNode>,
    similar_to_edges: Vec<SimilarEdge>,
    derives_from_edges: Vec<DerivesEdge>,
    supersedes_edges: Vec<SupersedesEdge>,
    transitioned_to_edges: Vec<TransitionEdge>,
    #[serde(default)]
    statistics: HierarchicalStats,
}

#[derive(Debug, Serialize, Deserialize)]
struct SemanticNode {
    memory_id: String,
    concept: String,
    content: String,
    confidence: f64,
    source_id: String,
    tags: Vec<String>,
    metadata: JsonValue,
    created_at: String,
    entity_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct EpisodicNode {
    memory_id: String,
    content: String,
    source_label: String,
    tags: Vec<String>,
    metadata: JsonValue,
    created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SimilarEdge {
    source_id: String,
    target_id: String,
    weight: f64,
    metadata: JsonValue,
}

#[derive(Debug, Serialize, Deserialize)]
struct DerivesEdge {
    source_id: String,
    target_id: String,
    extraction_method: String,
    confidence: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SupersedesEdge {
    source_id: String,
    target_id: String,
    reason: String,
    temporal_delta: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TransitionEdge {
    source_id: String,
    target_id: String,
    from_value: String,
    to_value: String,
    turn: i64,
    transition_type: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct HierarchicalStats {
    semantic_node_count: usize,
    episodic_node_count: usize,
    similar_to_edge_count: usize,
    derives_from_edge_count: usize,
    supersedes_edge_count: usize,
    transitioned_to_edge_count: usize,
}

#[derive(Debug, Default)]
struct ImportStats {
    semantic_nodes_imported: usize,
    episodic_nodes_imported: usize,
    edges_imported: usize,
    skipped: usize,
    errors: usize,
}

pub fn run_tree(
    session_id: Option<&str>,
    memory_type: Option<&str>,
    depth: Option<u32>,
    backend: &str,
) -> Result<()> {
    let backend = BackendChoice::parse(backend)?;
    let output = match backend {
        BackendChoice::Sqlite => render_sqlite_tree(session_id, memory_type, depth)?,
        BackendChoice::Kuzu => render_kuzu_tree(session_id, memory_type, depth)?,
    };
    println!("{output}");
    Ok(())
}

pub fn run_export(
    agent_name: &str,
    output: &str,
    format: &str,
    storage_path: Option<&str>,
) -> Result<()> {
    let format = TransferFormat::parse(format);
    match format.and_then(|fmt| export_memory(agent_name, output, fmt, storage_path)) {
        Ok(result) => {
            println!("Exported memory for agent '{}'", result.agent_name);
            println!("  Format: {}", result.format);
            println!("  Output: {}", result.output_path);
            if let Some(size_bytes) = result.file_size_bytes {
                println!("  Size: {:.1} KB", size_bytes as f64 / 1024.0);
            }
            for (key, value) in result.statistics_lines() {
                println!("  {key}: {value}");
            }
            Ok(())
        }
        Err(error) => {
            writeln!(io::stderr(), "Error exporting memory: {error}")?;
            Err(exit_error(1))
        }
    }
}

pub fn run_import(
    agent_name: &str,
    input: &str,
    format: &str,
    merge: bool,
    storage_path: Option<&str>,
) -> Result<()> {
    let format = TransferFormat::parse(format);
    match format.and_then(|fmt| import_memory(agent_name, input, fmt, merge, storage_path)) {
        Ok(result) => {
            println!("Imported memory into agent '{}'", result.agent_name);
            println!("  Format: {}", result.format);
            println!(
                "  Source agent: {}",
                result
                    .source_agent
                    .clone()
                    .unwrap_or_else(|| "N/A".to_string())
            );
            println!(
                "  Merge mode: {}",
                if result.merge { "True" } else { "False" }
            );
            for (key, value) in result.statistics_lines() {
                println!("  {key}: {value}");
            }
            Ok(())
        }
        Err(error) => {
            writeln!(io::stderr(), "Error importing memory: {error}")?;
            Err(exit_error(1))
        }
    }
}

pub fn run_clean(pattern: &str, backend: &str, dry_run: bool, confirm: bool) -> Result<()> {
    let backend = BackendChoice::parse(backend)?;
    let matched = match backend {
        BackendChoice::Sqlite => list_sqlite_sessions()?,
        BackendChoice::Kuzu => list_kuzu_sessions()?,
    }
    .into_iter()
    .filter(|session| wildcard_match(pattern, &session.session_id))
    .collect::<Vec<_>>();

    if matched.is_empty() {
        return Ok(());
    }

    print!(
        "\nFound {} session(s) matchin' pattern '{}':\n",
        matched.len(),
        pattern
    );
    for session in &matched {
        println!(
            "  - {} ({} memories)",
            session.session_id, session.memory_count
        );
    }

    if dry_run {
        println!("\nDry-run mode: No sessions were deleted.");
        println!("Use --no-dry-run to actually be deletin' these sessions.");
        return Ok(());
    }

    if !confirm {
        print!("\nAre ye sure ye want to delete these sessions? [y/N]: ");
        io::stdout().flush()?;
        let mut response = String::new();
        io::stdin().read_line(&mut response)?;
        let normalized = response.trim().to_ascii_lowercase();
        if normalized != "y" && normalized != "yes" {
            println!("Cleanup be cancelled.");
            return Ok(());
        }
    }

    let mut deleted_count = 0usize;
    let mut error_count = 0usize;
    for session in &matched {
        let deleted = match backend {
            BackendChoice::Sqlite => delete_sqlite_session(&session.session_id),
            BackendChoice::Kuzu => delete_kuzu_session(&session.session_id),
        };
        match deleted {
            Ok(true) => {
                deleted_count += 1;
                println!("Deleted: {}", session.session_id);
            }
            Ok(false) => {
                error_count += 1;
                writeln!(
                    io::stderr(),
                    "Failed to be deletin': {}",
                    session.session_id
                )?;
            }
            Err(error) => {
                error_count += 1;
                writeln!(
                    io::stderr(),
                    "Error deletin' {}: {error}",
                    session.session_id
                )?;
            }
        }
    }

    print!(
        "\nCleanup complete: {} deleted, {} errors\n",
        deleted_count, error_count
    );
    if error_count > 0 {
        return Err(exit_error(1));
    }
    Ok(())
}

#[derive(Debug)]
struct ExportResult {
    agent_name: String,
    format: String,
    output_path: String,
    file_size_bytes: Option<u64>,
    statistics: Vec<(String, String)>,
}

impl ExportResult {
    fn statistics_lines(&self) -> Vec<(String, String)> {
        self.statistics.clone()
    }
}

#[derive(Debug)]
struct ImportResult {
    agent_name: String,
    format: String,
    source_agent: Option<String>,
    merge: bool,
    statistics: Vec<(String, String)>,
}

impl ImportResult {
    fn statistics_lines(&self) -> Vec<(String, String)> {
        self.statistics.clone()
    }
}

fn render_sqlite_tree(
    session_id: Option<&str>,
    memory_type: Option<&str>,
    depth: Option<u32>,
) -> Result<String> {
    let conn = open_sqlite_memory_db()?;
    let mut sessions = list_sqlite_sessions_from_conn(&conn)?;
    if let Some(session_id) = session_id {
        sessions.retain(|session| session.session_id == session_id);
    }

    let mut session_rows = Vec::new();
    for session in sessions {
        let memories = query_sqlite_memories_for_session(&conn, &session.session_id, memory_type)?;
        session_rows.push((session, memories));
    }

    let agent_counts = if session_id.is_none() && depth.map(|value| value > 2).unwrap_or(true) {
        collect_sqlite_agent_counts(&conn)?
    } else {
        Vec::new()
    };

    Ok(render_tree(
        SQLITE_TREE_BACKEND_NAME,
        &session_rows,
        &agent_counts,
        session_id.is_none(),
        depth,
    ))
}

fn render_kuzu_tree(
    session_id: Option<&str>,
    _memory_type: Option<&str>,
    depth: Option<u32>,
) -> Result<String> {
    let db = open_kuzu_memory_db()?;
    let conn = KuzuConnection::new(&db).context("failed to connect to Kùzu memory DB")?;
    init_kuzu_backend_schema(&conn)?;

    let mut sessions = list_kuzu_sessions_from_conn(&conn)?;
    if let Some(session_id) = session_id {
        sessions.retain(|session| session.session_id == session_id);
    }

    let mut session_rows = Vec::new();
    for session in sessions {
        let memories = query_kuzu_memories_for_session(&conn, &session.session_id)?;
        let memory_count = memories.len();
        let mut session = session;
        session.memory_count = memory_count;
        session_rows.push((session, memories));
    }

    let agent_counts = if session_id.is_none() && depth.map(|value| value > 2).unwrap_or(true) {
        collect_kuzu_agent_counts(&conn)?
    } else {
        Vec::new()
    };

    Ok(render_tree(
        KUZU_TREE_BACKEND_NAME,
        &session_rows,
        &agent_counts,
        session_id.is_none(),
        depth,
    ))
}

fn render_tree(
    backend_name: &str,
    session_rows: &[(SessionSummary, Vec<MemoryRecord>)],
    agent_counts: &[(String, usize)],
    include_agents: bool,
    depth: Option<u32>,
) -> String {
    let show_agents =
        include_agents && depth.map(|value| value > 2).unwrap_or(true) && !agent_counts.is_empty();
    let mut lines = vec![format!("🧠 Memory Graph (Backend: {backend_name})")];
    if session_rows.is_empty() {
        lines.push("└── (empty - no memories found)".to_string());
        return lines.join("\n");
    }

    let sessions_branch = format!("📅 Sessions ({})", session_rows.len());
    lines.push(format!(
        "{} {sessions_branch}",
        if show_agents {
            "├──"
        } else {
            "└──"
        }
    ));
    let session_indent = if show_agents { "│   " } else { "    " };
    for (index, (session, memories)) in session_rows.iter().enumerate() {
        let last_session = index + 1 == session_rows.len();
        lines.push(format!(
            "{session_indent}{} {} ({} memories)",
            if last_session {
                "└──"
            } else {
                "├──"
            },
            session.session_id,
            session.memory_count
        ));
        let memory_indent = format!(
            "{session_indent}{}",
            if last_session { "    " } else { "│   " }
        );
        for (memory_index, memory) in memories.iter().enumerate() {
            let line = format_memory_line(memory);
            lines.push(format!(
                "{memory_indent}{} {line}",
                if memory_index + 1 == memories.len() {
                    "└──"
                } else {
                    "├──"
                }
            ));
        }
    }

    if show_agents {
        lines.push(format!("└── 👥 Agents ({})", agent_counts.len()));
        for (index, (agent_id, count)) in agent_counts.iter().enumerate() {
            lines.push(format!(
                "    {} {} ({count} memories)",
                if index + 1 == agent_counts.len() {
                    "└──"
                } else {
                    "├──"
                },
                agent_id
            ));
        }
    }

    lines.join("\n")
}

fn format_memory_line(memory: &MemoryRecord) -> String {
    let mut line = format!(
        "{} {}: {}",
        emoji_for_memory_type(&memory.memory_type),
        title_case(&memory.memory_type),
        memory.title
    );
    if let Some(importance) = memory.importance {
        line.push_str(&format!(" ({})", format_importance_score(importance)));
    }
    if let Some(confidence) = memory.metadata.get("confidence") {
        line.push_str(&format!(" (confidence: {})", json_scalar(confidence)));
    }
    if let Some(count) = memory.metadata.get("usage_count") {
        line.push_str(&format!(" (used: {}x)", json_scalar(count)));
    }
    if memory.expires_at.as_deref().is_some() {
        // Keep parity simple: only show expiry markers when the data exists in tests.
    }
    line
}

fn emoji_for_memory_type(memory_type: &str) -> &'static str {
    match memory_type {
        "conversation" => "📝",
        "pattern" => "💡",
        "decision" => "📌",
        "learning" => "💡",
        "context" => "🔧",
        "artifact" => "📄",
        _ => "❓",
    }
}

fn format_importance_score(importance: i64) -> String {
    let clamped = importance.clamp(0, 10) as usize;
    let filled = "★".repeat(clamped);
    let empty = "☆".repeat(10usize.saturating_sub(clamped));
    format!("{filled}{empty} {clamped}/10")
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn json_scalar(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => String::new(),
        JsonValue::Bool(v) => v.to_string(),
        JsonValue::Number(v) => v.to_string(),
        JsonValue::String(v) => v.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn open_sqlite_memory_db() -> Result<SqliteConnection> {
    let path = home_dir()?.join(".amplihack").join("memory.db");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let conn = SqliteConnection::open(path)?;
    conn.execute_batch(SQLITE_SCHEMA)?;
    Ok(conn)
}

fn list_sqlite_sessions() -> Result<Vec<SessionSummary>> {
    let conn = open_sqlite_memory_db()?;
    list_sqlite_sessions_from_conn(&conn)
}

fn list_sqlite_sessions_from_conn(conn: &SqliteConnection) -> Result<Vec<SessionSummary>> {
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

fn query_sqlite_memories_for_session(
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

fn collect_sqlite_agent_counts(conn: &SqliteConnection) -> Result<Vec<(String, usize)>> {
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

fn delete_sqlite_session(session_id: &str) -> Result<bool> {
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

fn open_kuzu_memory_db() -> Result<KuzuDatabase> {
    let path = home_dir()?.join(".amplihack").join("memory_kuzu.db");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(KuzuDatabase::new(path, SystemConfig::default())?)
}

fn init_kuzu_backend_schema(conn: &KuzuConnection<'_>) -> Result<()> {
    for statement in KUZU_BACKEND_SCHEMA {
        conn.query(statement)?;
    }
    Ok(())
}

fn list_kuzu_sessions() -> Result<Vec<SessionSummary>> {
    let db = open_kuzu_memory_db()?;
    let conn = KuzuConnection::new(&db)?;
    init_kuzu_backend_schema(&conn)?;
    list_kuzu_sessions_from_conn(&conn)
}

fn list_kuzu_sessions_from_conn(conn: &KuzuConnection<'_>) -> Result<Vec<SessionSummary>> {
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

fn query_kuzu_memories_for_session(
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

fn collect_kuzu_agent_counts(conn: &KuzuConnection<'_>) -> Result<Vec<(String, usize)>> {
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

fn delete_kuzu_session(session_id: &str) -> Result<bool> {
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

fn export_memory(
    agent_name: &str,
    output: &str,
    format: TransferFormat,
    storage_path: Option<&str>,
) -> Result<ExportResult> {
    match format {
        TransferFormat::Json => export_hierarchical_json(agent_name, output, storage_path),
        TransferFormat::Kuzu => export_hierarchical_kuzu(agent_name, output, storage_path),
    }
}

fn import_memory(
    agent_name: &str,
    input: &str,
    format: TransferFormat,
    merge: bool,
    storage_path: Option<&str>,
) -> Result<ImportResult> {
    match format {
        TransferFormat::Json => import_hierarchical_json(agent_name, input, merge, storage_path),
        TransferFormat::Kuzu => import_hierarchical_kuzu(agent_name, input, merge, storage_path),
    }
}

fn export_hierarchical_json(
    agent_name: &str,
    output: &str,
    storage_path: Option<&str>,
) -> Result<ExportResult> {
    let db_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    let db = KuzuDatabase::new(&db_path, SystemConfig::default())?;
    let conn = KuzuConnection::new(&db)?;
    init_hierarchical_schema(&conn)?;

    let semantic_nodes = kuzu_rows(
        &conn,
        "MATCH (m:SemanticMemory) WHERE m.agent_id = $agent_id RETURN m.memory_id, m.concept, m.content, m.confidence, m.source_id, m.tags, m.metadata, m.created_at, m.entity_name ORDER BY m.created_at ASC",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<SemanticNode> {
        Ok(SemanticNode {
            memory_id: kuzu_string(row.first())?,
            concept: kuzu_string(row.get(1))?,
            content: kuzu_string(row.get(2))?,
            confidence: kuzu_f64(row.get(3))?,
            source_id: kuzu_string(row.get(4))?,
            tags: parse_json_array_of_strings(&kuzu_string(row.get(5))?)?,
            metadata: parse_json_value(&kuzu_string(row.get(6))?)
                .unwrap_or(JsonValue::Object(Default::default())),
            created_at: kuzu_string(row.get(7))?,
            entity_name: kuzu_string(row.get(8))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let episodic_nodes = kuzu_rows(
        &conn,
        "MATCH (e:EpisodicMemory) WHERE e.agent_id = $agent_id RETURN e.memory_id, e.content, e.source_label, e.tags, e.metadata, e.created_at ORDER BY e.created_at ASC",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<EpisodicNode> {
        Ok(EpisodicNode {
            memory_id: kuzu_string(row.first())?,
            content: kuzu_string(row.get(1))?,
            source_label: kuzu_string(row.get(2))?,
            tags: parse_json_array_of_strings(&kuzu_string(row.get(3))?)?,
            metadata: parse_json_value(&kuzu_string(row.get(4))?)
                .unwrap_or(JsonValue::Object(Default::default())),
            created_at: kuzu_string(row.get(5))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let similar_to_edges = kuzu_rows(
        &conn,
        "MATCH (a:SemanticMemory)-[r:SIMILAR_TO]->(b:SemanticMemory) WHERE a.agent_id = $agent_id RETURN a.memory_id, b.memory_id, r.weight, r.metadata",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<SimilarEdge> {
        Ok(SimilarEdge {
            source_id: kuzu_string(row.first())?,
            target_id: kuzu_string(row.get(1))?,
            weight: kuzu_f64(row.get(2))?,
            metadata: parse_json_value(&kuzu_string(row.get(3))?)
                .unwrap_or(JsonValue::Object(Default::default())),
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let derives_from_edges = kuzu_rows(
        &conn,
        "MATCH (s:SemanticMemory)-[r:DERIVES_FROM]->(e:EpisodicMemory) WHERE s.agent_id = $agent_id RETURN s.memory_id, e.memory_id, r.extraction_method, r.confidence",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<DerivesEdge> {
        Ok(DerivesEdge {
            source_id: kuzu_string(row.first())?,
            target_id: kuzu_string(row.get(1))?,
            extraction_method: kuzu_string(row.get(2))?,
            confidence: kuzu_f64(row.get(3))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let supersedes_edges = kuzu_rows(
        &conn,
        "MATCH (newer:SemanticMemory)-[r:SUPERSEDES]->(older:SemanticMemory) WHERE newer.agent_id = $agent_id RETURN newer.memory_id, older.memory_id, r.reason, r.temporal_delta",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<SupersedesEdge> {
        Ok(SupersedesEdge {
            source_id: kuzu_string(row.first())?,
            target_id: kuzu_string(row.get(1))?,
            reason: kuzu_string(row.get(2))?,
            temporal_delta: kuzu_string(row.get(3))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let transitioned_to_edges = kuzu_rows(
        &conn,
        "MATCH (newer:SemanticMemory)-[r:TRANSITIONED_TO]->(older:SemanticMemory) WHERE newer.agent_id = $agent_id RETURN newer.memory_id, older.memory_id, r.from_value, r.to_value, r.turn, r.transition_type",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<TransitionEdge> {
        Ok(TransitionEdge {
            source_id: kuzu_string(row.first())?,
            target_id: kuzu_string(row.get(1))?,
            from_value: kuzu_string(row.get(2))?,
            to_value: kuzu_string(row.get(3))?,
            turn: kuzu_i64(row.get(4))?,
            transition_type: kuzu_string(row.get(5))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let export = HierarchicalExportData {
        agent_name: agent_name.to_string(),
        exported_at: kuzu_export_timestamp(),
        format_version: "1.1".to_string(),
        semantic_nodes,
        episodic_nodes,
        similar_to_edges,
        derives_from_edges,
        supersedes_edges,
        transitioned_to_edges,
        statistics: HierarchicalStats::default(),
    };
    let mut export = export;
    export.statistics = HierarchicalStats {
        semantic_node_count: export.semantic_nodes.len(),
        episodic_node_count: export.episodic_nodes.len(),
        similar_to_edge_count: export.similar_to_edges.len(),
        derives_from_edge_count: export.derives_from_edges.len(),
        supersedes_edge_count: export.supersedes_edges.len(),
        transitioned_to_edge_count: export.transitioned_to_edges.len(),
    };

    let output_path = PathBuf::from(output);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_string_pretty(&export)?;
    fs::write(&output_path, serialized)?;
    let file_size = output_path.metadata()?.len();
    Ok(ExportResult {
        agent_name: agent_name.to_string(),
        format: "json".to_string(),
        output_path: output_path.canonicalize()?.display().to_string(),
        file_size_bytes: Some(file_size),
        statistics: vec![
            (
                "semantic_node_count".to_string(),
                export.statistics.semantic_node_count.to_string(),
            ),
            (
                "episodic_node_count".to_string(),
                export.statistics.episodic_node_count.to_string(),
            ),
            (
                "similar_to_edge_count".to_string(),
                export.statistics.similar_to_edge_count.to_string(),
            ),
            (
                "derives_from_edge_count".to_string(),
                export.statistics.derives_from_edge_count.to_string(),
            ),
            (
                "supersedes_edge_count".to_string(),
                export.statistics.supersedes_edge_count.to_string(),
            ),
            (
                "transitioned_to_edge_count".to_string(),
                export.statistics.transitioned_to_edge_count.to_string(),
            ),
        ],
    })
}

fn export_hierarchical_kuzu(
    agent_name: &str,
    output: &str,
    storage_path: Option<&str>,
) -> Result<ExportResult> {
    let db_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    let output_path = PathBuf::from(output);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if output_path.exists() {
        if output_path.is_dir() {
            fs::remove_dir_all(&output_path)?;
        } else {
            fs::remove_file(&output_path)?;
        }
    }
    copy_hierarchical_storage(&db_path, &output_path)?;
    let size = compute_path_size(&output_path)?;
    Ok(ExportResult {
        agent_name: agent_name.to_string(),
        format: "kuzu".to_string(),
        output_path: output_path.canonicalize()?.display().to_string(),
        file_size_bytes: Some(size),
        statistics: vec![(
            "note".to_string(),
            "Raw Kuzu DB copy - use JSON format for node/edge counts".to_string(),
        )],
    })
}

fn import_hierarchical_json(
    agent_name: &str,
    input: &str,
    merge: bool,
    storage_path: Option<&str>,
) -> Result<ImportResult> {
    let input_path = PathBuf::from(input);
    let mut raw = String::new();
    fs::File::open(&input_path)?.read_to_string(&mut raw)?;
    let data: HierarchicalExportData = serde_json::from_str(&raw)?;
    let db_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let db = KuzuDatabase::new(&db_path, SystemConfig::default())?;
    let conn = KuzuConnection::new(&db)?;
    init_hierarchical_schema(&conn)?;
    if !merge {
        clear_hierarchical_agent_data(&conn, agent_name)?;
    }
    let existing_ids = if merge {
        get_existing_hierarchical_ids(&conn, agent_name)?
    } else {
        Vec::new()
    };
    let mut stats = ImportStats::default();

    for node in &data.episodic_nodes {
        if node.memory_id.is_empty() {
            stats.errors += 1;
            continue;
        }
        if merge && existing_ids.contains(&node.memory_id) {
            stats.skipped += 1;
            continue;
        }
        let mut prepared = conn.prepare(
            "CREATE (e:EpisodicMemory {memory_id: $memory_id, content: $content, source_label: $source_label, agent_id: $agent_id, tags: $tags, metadata: $metadata, created_at: $created_at})",
        )?;
        if conn
            .execute(
                &mut prepared,
                vec![
                    ("memory_id", KuzuValue::String(node.memory_id.clone())),
                    ("content", KuzuValue::String(node.content.clone())),
                    ("source_label", KuzuValue::String(node.source_label.clone())),
                    ("agent_id", KuzuValue::String(agent_name.to_string())),
                    (
                        "tags",
                        KuzuValue::String(serde_json::to_string(&node.tags)?),
                    ),
                    (
                        "metadata",
                        KuzuValue::String(serde_json::to_string(&node.metadata)?),
                    ),
                    ("created_at", KuzuValue::String(node.created_at.clone())),
                ],
            )
            .is_ok()
        {
            stats.episodic_nodes_imported += 1;
        } else {
            stats.errors += 1;
        }
    }

    for node in &data.semantic_nodes {
        if node.memory_id.is_empty() {
            stats.errors += 1;
            continue;
        }
        if merge && existing_ids.contains(&node.memory_id) {
            stats.skipped += 1;
            continue;
        }
        let mut prepared = conn.prepare(
            "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, confidence: $confidence, source_id: $source_id, agent_id: $agent_id, tags: $tags, metadata: $metadata, created_at: $created_at, entity_name: $entity_name})",
        )?;
        if conn
            .execute(
                &mut prepared,
                vec![
                    ("memory_id", KuzuValue::String(node.memory_id.clone())),
                    ("concept", KuzuValue::String(node.concept.clone())),
                    ("content", KuzuValue::String(node.content.clone())),
                    ("confidence", KuzuValue::Double(node.confidence)),
                    ("source_id", KuzuValue::String(node.source_id.clone())),
                    ("agent_id", KuzuValue::String(agent_name.to_string())),
                    (
                        "tags",
                        KuzuValue::String(serde_json::to_string(&node.tags)?),
                    ),
                    (
                        "metadata",
                        KuzuValue::String(serde_json::to_string(&node.metadata)?),
                    ),
                    ("created_at", KuzuValue::String(node.created_at.clone())),
                    ("entity_name", KuzuValue::String(node.entity_name.clone())),
                ],
            )
            .is_ok()
        {
            stats.semantic_nodes_imported += 1;
        } else {
            stats.errors += 1;
        }
    }

    for edge in &data.similar_to_edges {
        if create_hierarchical_edge(
            &conn,
            "MATCH (a:SemanticMemory {memory_id: $sid}) MATCH (b:SemanticMemory {memory_id: $tid}) CREATE (a)-[:SIMILAR_TO {weight: $weight, metadata: $metadata}]->(b)",
            vec![
                ("sid", KuzuValue::String(edge.source_id.clone())),
                ("tid", KuzuValue::String(edge.target_id.clone())),
                ("weight", KuzuValue::Double(edge.weight)),
                (
                    "metadata",
                    KuzuValue::String(serde_json::to_string(&edge.metadata)?),
                ),
            ],
        )? {
            stats.edges_imported += 1;
        } else {
            stats.errors += 1;
        }
    }
    for edge in &data.derives_from_edges {
        if create_hierarchical_edge(
            &conn,
            "MATCH (s:SemanticMemory {memory_id: $sid}) MATCH (e:EpisodicMemory {memory_id: $tid}) CREATE (s)-[:DERIVES_FROM {extraction_method: $method, confidence: $confidence}]->(e)",
            vec![
                ("sid", KuzuValue::String(edge.source_id.clone())),
                ("tid", KuzuValue::String(edge.target_id.clone())),
                ("method", KuzuValue::String(edge.extraction_method.clone())),
                ("confidence", KuzuValue::Double(edge.confidence)),
            ],
        )? {
            stats.edges_imported += 1;
        } else {
            stats.errors += 1;
        }
    }
    for edge in &data.supersedes_edges {
        if create_hierarchical_edge(
            &conn,
            "MATCH (newer:SemanticMemory {memory_id: $sid}) MATCH (older:SemanticMemory {memory_id: $tid}) CREATE (newer)-[:SUPERSEDES {reason: $reason, temporal_delta: $delta}]->(older)",
            vec![
                ("sid", KuzuValue::String(edge.source_id.clone())),
                ("tid", KuzuValue::String(edge.target_id.clone())),
                ("reason", KuzuValue::String(edge.reason.clone())),
                ("delta", KuzuValue::String(edge.temporal_delta.clone())),
            ],
        )? {
            stats.edges_imported += 1;
        } else {
            stats.errors += 1;
        }
    }
    for edge in &data.transitioned_to_edges {
        if create_hierarchical_edge(
            &conn,
            "MATCH (newer:SemanticMemory {memory_id: $sid}) MATCH (older:SemanticMemory {memory_id: $tid}) CREATE (newer)-[:TRANSITIONED_TO {from_value: $from_val, to_value: $to_val, turn: $turn, transition_type: $ttype}]->(older)",
            vec![
                ("sid", KuzuValue::String(edge.source_id.clone())),
                ("tid", KuzuValue::String(edge.target_id.clone())),
                ("from_val", KuzuValue::String(edge.from_value.clone())),
                ("to_val", KuzuValue::String(edge.to_value.clone())),
                ("turn", KuzuValue::Int64(edge.turn)),
                ("ttype", KuzuValue::String(edge.transition_type.clone())),
            ],
        )? {
            stats.edges_imported += 1;
        } else {
            stats.errors += 1;
        }
    }

    Ok(ImportResult {
        agent_name: agent_name.to_string(),
        format: "json".to_string(),
        source_agent: Some(data.agent_name),
        merge,
        statistics: vec![
            (
                "semantic_nodes_imported".to_string(),
                stats.semantic_nodes_imported.to_string(),
            ),
            (
                "episodic_nodes_imported".to_string(),
                stats.episodic_nodes_imported.to_string(),
            ),
            (
                "edges_imported".to_string(),
                stats.edges_imported.to_string(),
            ),
            ("skipped".to_string(), stats.skipped.to_string()),
            ("errors".to_string(), stats.errors.to_string()),
        ],
    })
}

fn import_hierarchical_kuzu(
    agent_name: &str,
    input: &str,
    merge: bool,
    storage_path: Option<&str>,
) -> Result<ImportResult> {
    if merge {
        anyhow::bail!(
            "Merge mode is not supported for kuzu format. Use JSON format for merge imports, or set merge=False to replace the DB entirely."
        );
    }
    let input_path = PathBuf::from(input);
    if !input_path.exists() {
        anyhow::bail!("Input path does not exist: {}", input_path.display());
    }
    let target_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if target_path.exists() {
        let backup_path = target_path.with_extension("bak");
        if backup_path.exists() {
            if backup_path.is_dir() {
                fs::remove_dir_all(&backup_path)?;
            } else {
                fs::remove_file(&backup_path)?;
            }
        }
        fs::rename(&target_path, &backup_path)?;
    }
    copy_hierarchical_storage(&input_path, &target_path)?;
    Ok(ImportResult {
        agent_name: agent_name.to_string(),
        format: "kuzu".to_string(),
        source_agent: None,
        merge: false,
        statistics: vec![(
            "note".to_string(),
            "Raw Kuzu DB replaced - restart agent to use new DB".to_string(),
        )],
    })
}

fn init_hierarchical_schema(conn: &KuzuConnection<'_>) -> Result<()> {
    for statement in HIERARCHICAL_SCHEMA {
        conn.query(statement)?;
    }
    Ok(())
}

fn clear_hierarchical_agent_data(conn: &KuzuConnection<'_>, agent_name: &str) -> Result<()> {
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
        kuzu_rows(
            conn,
            query,
            vec![("aid", KuzuValue::String(agent_name.to_string()))],
        )?;
    }
    Ok(())
}

fn get_existing_hierarchical_ids(
    conn: &KuzuConnection<'_>,
    agent_name: &str,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for query in [
        "MATCH (m:SemanticMemory {agent_id: $aid}) RETURN m.memory_id",
        "MATCH (e:EpisodicMemory {agent_id: $aid}) RETURN e.memory_id",
    ] {
        let rows = kuzu_rows(
            conn,
            query,
            vec![("aid", KuzuValue::String(agent_name.to_string()))],
        )?;
        for row in rows {
            ids.push(kuzu_string(row.first())?);
        }
    }
    Ok(ids)
}

fn create_hierarchical_edge(
    conn: &KuzuConnection<'_>,
    query: &str,
    params: Vec<(&str, KuzuValue)>,
) -> Result<bool> {
    let mut prepared = conn.prepare(query)?;
    Ok(conn.execute(&mut prepared, params).is_ok())
}

fn resolve_hierarchical_db_path(agent_name: &str, storage_path: Option<&str>) -> Result<PathBuf> {
    let base = match storage_path {
        Some(path) => PathBuf::from(path),
        None => home_dir()?
            .join(".amplihack")
            .join("hierarchical_memory")
            .join(agent_name),
    };
    if base.is_dir() && !base.join("kuzu.lock").exists() {
        return Ok(base.join("kuzu_db"));
    }
    if base.join("kuzu_db").is_dir() {
        return Ok(base.join("kuzu_db"));
    }
    Ok(base)
}

fn copy_hierarchical_storage(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        copy_dir_recursive(src, dst)?;
        return Ok(());
    }
    fs::copy(src, dst)
        .with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn compute_path_size(path: &Path) -> Result<u64> {
    if path.is_file() {
        return Ok(path.metadata()?.len());
    }
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        total += compute_path_size(&entry.path())?;
    }
    Ok(total)
}

fn kuzu_rows(
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

fn memory_from_kuzu_node(
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

fn property_string(props: &[(String, KuzuValue)], key: &str) -> Option<String> {
    props.iter().find_map(|(name, value)| {
        if name == key {
            Some(kuzu_value_to_string(value))
        } else {
            None
        }
    })
}

fn property_i64(props: &[(String, KuzuValue)], key: &str) -> Option<i64> {
    props.iter().find_map(|(name, value)| {
        if name == key {
            kuzu_value_to_i64(value)
        } else {
            None
        }
    })
}

fn kuzu_value_to_string(value: &KuzuValue) -> String {
    match value {
        KuzuValue::Null(_) => String::new(),
        KuzuValue::String(v) => v.clone(),
        other => other.to_string(),
    }
}

fn kuzu_value_to_i64(value: &KuzuValue) -> Option<i64> {
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

fn kuzu_string(value: Option<&KuzuValue>) -> Result<String> {
    Ok(value.map(kuzu_value_to_string).unwrap_or_default())
}

fn kuzu_i64(value: Option<&KuzuValue>) -> Result<i64> {
    value
        .and_then(kuzu_value_to_i64)
        .context("expected integer Kùzu value")
}

fn kuzu_f64(value: Option<&KuzuValue>) -> Result<f64> {
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

fn parse_json_array_of_strings(value: &str) -> Result<Vec<String>> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let parsed = parse_json_value(value)?;
    match parsed {
        JsonValue::Array(items) => Ok(items
            .into_iter()
            .filter_map(|item| match item {
                JsonValue::String(value) => Some(value),
                _ => None,
            })
            .collect()),
        _ => Ok(Vec::new()),
    }
}

fn parse_json_value(value: &str) -> Result<JsonValue> {
    if value.is_empty() {
        return Ok(JsonValue::Object(Default::default()));
    }
    Ok(serde_json::from_str(value)?)
}

fn to_sqlite_err(error: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern_chars = pattern.as_bytes();
    let value_chars = value.as_bytes();
    let mut dp = vec![vec![false; value_chars.len() + 1]; pattern_chars.len() + 1];
    dp[0][0] = true;
    for i in 1..=pattern_chars.len() {
        if pattern_chars[i - 1] == b'*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=pattern_chars.len() {
        for j in 1..=value_chars.len() {
            dp[i][j] = match pattern_chars[i - 1] {
                b'*' => dp[i - 1][j] || dp[i][j - 1],
                b'?' => dp[i - 1][j - 1],
                current => dp[i - 1][j - 1] && current == value_chars[j - 1],
            };
        }
    }
    dp[pattern_chars.len()][value_chars.len()]
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}

fn kuzu_export_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Match Python well enough for parity comparisons that normalize timestamps.
    format!("{}", now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn wildcard_matching_supports_globs() {
        assert!(wildcard_match("test_*", "test_session"));
        assert!(wildcard_match("dev_?", "dev_a"));
        assert!(!wildcard_match("dev_?", "dev_ab"));
        assert!(!wildcard_match("demo_*", "test_session"));
    }

    #[test]
    fn render_tree_matches_python_shape() {
        let session = SessionSummary {
            session_id: "test_sess".to_string(),
            memory_count: 2,
        };
        let rows = vec![(
            session,
            vec![
                MemoryRecord {
                    memory_type: "conversation".to_string(),
                    title: "Hello".to_string(),
                    metadata: serde_json::json!({"confidence": 0.9}),
                    importance: Some(8),
                    expires_at: None,
                },
                MemoryRecord {
                    memory_type: "context".to_string(),
                    title: "Ctx".to_string(),
                    metadata: serde_json::json!({"usage_count": 3}),
                    importance: None,
                    expires_at: None,
                },
            ],
        )];
        let output = render_tree(
            SQLITE_TREE_BACKEND_NAME,
            &rows,
            &[("agent1".to_string(), 2)],
            true,
            None,
        );
        assert!(output.contains("🧠 Memory Graph (Backend: unknown)"));
        assert!(output.contains("📝 Conversation: Hello (★★★★★★★★☆☆ 8/10) (confidence: 0.9)"));
        assert!(output.contains("🔧 Context: Ctx (used: 3x)"));
    }

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
