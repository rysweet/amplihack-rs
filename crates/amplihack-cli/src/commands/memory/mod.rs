//! Native memory commands (`tree`, `export`, `import`, `clean`).

pub mod backend;
pub mod clean;
pub mod code_graph;
pub mod indexing_job;
pub mod scip_indexing;
pub mod staleness_detector;
pub mod transfer;
pub mod tree;

#[cfg(test)]
pub(crate) use backend::sqlite::{
    SQLITE_SCHEMA, SQLITE_TREE_BACKEND_NAME, list_sqlite_sessions_from_conn, open_sqlite_memory_db,
};
pub use clean::run_clean;
pub use code_graph::{
    CodeGraphSummary, code_graph_compatibility_notice_for_project,
    default_code_graph_db_path_for_project, import_scip_file,
    resolve_code_graph_db_path_for_project, run_index_code, summarize_code_graph,
};
pub use indexing_job::{
    background_index_job_active, background_index_job_path, record_background_index_pid,
};
pub use scip_indexing::{
    check_prerequisites, detect_project_languages, run_index_scip, run_native_scip_indexing,
};
pub use staleness_detector::{IndexStatus, check_index_status};
pub use transfer::{run_export, run_import};
pub use tree::run_tree;

use self::backend::kuzu::resolve_memory_graph_db_path;
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(crate) const GRAPH_DB_TREE_BACKEND_NAME: &str = "graph-db";
pub(crate) const HIERARCHICAL_SCHEMA: &[&str] = &[
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendChoice {
    GraphDb,
    Sqlite,
}

impl BackendChoice {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "graph-db" | "kuzu" => Ok(Self::GraphDb),
            "sqlite" => Ok(Self::Sqlite),
            other => anyhow::bail!("Invalid backend: {other}. Must be graph-db or sqlite"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferFormat {
    Json,
    RawDb,
}

impl TransferFormat {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "json" => Ok(Self::Json),
            "raw-db" | "kuzu" => Ok(Self::RawDb),
            other => anyhow::bail!("Unsupported format: {other:?}. Use one of: ('json', 'raw-db')"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub memory_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryRecord {
    pub(crate) memory_id: String,
    pub(crate) memory_type: String,
    pub(crate) title: String,
    pub(crate) content: String,
    pub(crate) metadata: JsonValue,
    pub(crate) importance: Option<i64>,
    pub(crate) accessed_at: Option<String>,
    pub(crate) expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContextMemory {
    pub content: String,
    pub code_context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectedPromptContextMemory {
    memory_id: String,
    content: String,
    code_context: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionLearningRecord {
    session_id: String,
    agent_id: String,
    content: String,
    title: String,
    metadata: JsonValue,
    importance: i64,
}

pub(crate) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}

pub(crate) fn parse_json_value(value: &str) -> Result<JsonValue> {
    if value.is_empty() {
        return Ok(JsonValue::Object(Default::default()));
    }
    Ok(serde_json::from_str(value)?)
}

fn resolve_memory_backend_preference() -> Option<BackendChoice> {
    match std::env::var("AMPLIHACK_MEMORY_BACKEND").ok().as_deref() {
        Some("sqlite") => Some(BackendChoice::Sqlite),
        Some("graph-db") | Some("kuzu") => Some(BackendChoice::GraphDb),
        _ => None,
    }
}

pub(crate) fn memory_graph_compatibility_notice(choice: BackendChoice) -> Option<String> {
    if !matches!(choice, BackendChoice::GraphDb) {
        return None;
    }

    let graph_override = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    if graph_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return None;
    }

    let legacy_override = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    if legacy_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Some(
            "using legacy `AMPLIHACK_KUZU_DB_PATH`; prefer `AMPLIHACK_GRAPH_DB_PATH`.".to_string(),
        );
    }

    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let neutral = home.join(".amplihack").join("memory_graph.db");
    let legacy = home.join(".amplihack").join("memory_kuzu.db");
    if legacy.exists() && !neutral.exists() {
        return Some(format!(
            "using legacy store `{}` because `{}` is absent; migrate to `memory_graph.db`.",
            legacy.display(),
            neutral.display()
        ));
    }

    None
}

/// Resolve the memory backend with autodetection.
///
/// Resolution order:
/// 1. `AMPLIHACK_MEMORY_BACKEND` env var (if set and recognized).
/// 2. Probe `~/.amplihack/hierarchical_memory/` for existing Kuzu `graph_db`
///    directories using `symlink_metadata()` (not `exists()`).
///    - If a symlink is found inside the probe directory → return `Err`.
///    - If a `graph_db` subdirectory is found → `BackendChoice::GraphDb`.
/// 3. Default to `BackendChoice::Sqlite` for new installs.
///
/// Returns `Err` if `HOME` is unavailable (only checked when the env var
/// shortcut is not used).
pub(crate) fn resolve_backend_with_autodetect() -> Result<BackendChoice> {
    // Step 1: env var takes priority.
    if let Some(choice) = resolve_memory_backend_preference() {
        return Ok(choice);
    }

    // Step 2: probe the filesystem.
    let home = home_dir()?;
    let hmem_dir = home.join(".amplihack").join("hierarchical_memory");

    // If the directory doesn't exist at all, this is a fresh install.
    if hmem_dir.symlink_metadata().is_err() {
        return Ok(BackendChoice::Sqlite);
    }

    // Scan the hierarchical_memory directory for agent subdirectories.
    // Use symlink_metadata() on each entry to detect symlinks.
    for entry_result in std::fs::read_dir(&hmem_dir)
        .with_context(|| format!("failed to read directory {}", hmem_dir.display()))?
    {
        let entry = entry_result
            .with_context(|| format!("failed to read entry in {}", hmem_dir.display()))?;
        let entry_path = entry.path();

        // Use symlink_metadata() to detect symlinks without following them.
        let meta = entry_path
            .symlink_metadata()
            .with_context(|| format!("failed to stat {}", entry_path.display()))?;

        if meta.file_type().is_symlink() {
            anyhow::bail!(
                "symlink detected in backend probe path {}; refusing to follow for security",
                entry_path.display()
            );
        }

        if meta.is_dir() {
            // Check if this agent directory contains a graph_db subdirectory.
            let graph_db = entry_path.join("graph_db");
            if graph_db.symlink_metadata().is_ok() {
                return Ok(BackendChoice::GraphDb);
            }
        }
    }

    // Step 3: No Kuzu markers found → default to SQLite.
    Ok(BackendChoice::Sqlite)
}

fn load_runtime_memories_from_backend(
    choice: BackendChoice,
    session_id: &str,
) -> Result<Vec<MemoryRecord>> {
    self::backend::open_runtime_backend(choice)?.load_prompt_context_memories(session_id)
}

fn is_prompt_context_memory(memory: &MemoryRecord) -> bool {
    matches!(
        memory
            .metadata
            .get("new_memory_type")
            .and_then(JsonValue::as_str),
        Some("episodic" | "semantic" | "procedural")
    )
}

pub(crate) fn parse_memory_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc())
        })
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc())
        })
        .or_else(|| {
            // Handle time::OffsetDateTime Display format: "{date} {time} {offset}"
            // e.g. "1971-01-01 0:00:00.0 +00:00:00"
            // Strip the timezone offset token (third space-delimited part) and
            // parse the date+time as UTC.
            let mut parts = value.splitn(3, ' ');
            if let (Some(date), Some(time_str), Some(_offset)) =
                (parts.next(), parts.next(), parts.next())
            {
                let candidate = format!("{date} {time_str}");
                NaiveDateTime::parse_from_str(&candidate, "%Y-%m-%d %H:%M:%S%.f")
                    .ok()
                    .map(|dt| dt.and_utc())
            } else {
                None
            }
        })
}

/// Score a single memory record against a pre-lowercased query string.
///
/// `query_lower` **must** already be lowercase.  `query_words` must be the
/// set of whitespace-split tokens from `query_lower`, also pre-computed by
/// the caller so it is not re-allocated once per memory record.
fn memory_relevance_score(
    memory: &MemoryRecord,
    query_lower: &str,
    query_words: &HashSet<&str>,
) -> f64 {
    let content_lower = memory.content.to_lowercase();
    let mut score = 0.0;

    if !query_lower.is_empty() && content_lower.contains(query_lower) {
        score += 10.0;
    }

    let content_words: HashSet<&str> = content_lower.split_whitespace().collect();
    score += query_words.intersection(&content_words).count() as f64 * 2.0;

    if let Some(accessed_at) = memory.accessed_at.as_deref()
        && let Some(timestamp) = parse_memory_timestamp(accessed_at)
    {
        let age_days = (Utc::now() - timestamp).num_days().max(0) as f64;
        score += (5.0 - (age_days * 0.1)).max(0.0);
    }

    if let Some(importance) = memory.importance {
        score += importance as f64;
    }

    score
}

fn select_prompt_context_memories(
    memories: Vec<MemoryRecord>,
    query_text: &str,
    token_budget: usize,
) -> Vec<SelectedPromptContextMemory> {
    if token_budget == 0 {
        return Vec::new();
    }

    // Pre-compute once: lower-cased query string and its word set.
    // `memory_relevance_score` accepts both so neither is rebuilt per record.
    let query_lower = query_text.to_lowercase();
    let query_words: HashSet<&str> = query_lower.split_whitespace().collect();

    let mut ranked = memories
        .into_iter()
        .filter(is_prompt_context_memory)
        .map(|memory| {
            let score = memory_relevance_score(&memory, &query_lower, &query_words);
            (memory, score)
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));

    let mut total_tokens = 0usize;
    let mut selected = Vec::new();
    for (memory, _) in ranked {
        // Use byte length / 4 as a token budget approximation — identical to
        // the previous chars().count() / 4 for ASCII, a slight overestimate
        // for multibyte UTF-8, and O(1) instead of O(n).
        let memory_tokens = memory.content.len() / 4;
        if total_tokens + memory_tokens > token_budget {
            break;
        }
        selected.push(SelectedPromptContextMemory {
            memory_id: memory.memory_id,
            content: memory.content,
            code_context: None,
        });
        total_tokens += memory_tokens;
    }

    selected
}

fn format_code_context(payload: &code_graph::CodeGraphContextPayload) -> Option<String> {
    if payload.files.is_empty() && payload.functions.is_empty() && payload.classes.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    if !payload.files.is_empty() {
        lines.push("**Related Files:**".to_string());
        for file in payload.files.iter().take(5) {
            lines.push(format!("- {} ({})", file.path, file.language));
        }
    }

    if !payload.functions.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("**Related Functions:**".to_string());
        for function in payload.functions.iter().take(5) {
            let signature = if function.signature.trim().is_empty() {
                function.name.as_str()
            } else {
                function.signature.as_str()
            };
            lines.push(format!("- `{}`", signature));
            if !function.docstring.trim().is_empty() {
                let doc_preview = if function.docstring.chars().count() > 100 {
                    let truncated = function.docstring.chars().take(100).collect::<String>();
                    format!("{truncated}...")
                } else {
                    function.docstring.clone()
                };
                lines.push(format!("  {doc_preview}"));
            }
            if function.complexity > 0 {
                lines.push(format!("  (complexity: {})", function.complexity));
            }
        }
    }

    if !payload.classes.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("**Related Classes:**".to_string());
        for class in payload.classes.iter().take(3) {
            let name = if class.fully_qualified_name.trim().is_empty() {
                class.name.as_str()
            } else {
                class.fully_qualified_name.as_str()
            };
            lines.push(format!("- {}", name));
            if !class.docstring.trim().is_empty() {
                let doc_preview = if class.docstring.chars().count() > 100 {
                    let truncated = class.docstring.chars().take(100).collect::<String>();
                    format!("{truncated}...")
                } else {
                    class.docstring.clone()
                };
                lines.push(format!("  {doc_preview}"));
            }
        }
    }

    Some(lines.join("\n"))
}

fn enrich_prompt_context_memories_with_reader(
    selected: Vec<SelectedPromptContextMemory>,
    reader: &dyn code_graph::CodeGraphReaderBackend,
) -> Result<Vec<SelectedPromptContextMemory>> {
    if selected.is_empty() {
        return Ok(selected);
    }

    let mut enriched = Vec::with_capacity(selected.len());
    for mut memory in selected {
        if memory.memory_id.trim().is_empty() {
            enriched.push(memory);
            continue;
        }

        let payload = reader.context_payload(&memory.memory_id).with_context(|| {
            format!(
                "failed to load prompt memory code context for {}",
                memory.memory_id
            )
        })?;
        memory.code_context = format_code_context(&payload);
        enriched.push(memory);
    }
    Ok(enriched)
}

fn enrich_prompt_context_memories_with_code_context_at_path(
    selected: Vec<SelectedPromptContextMemory>,
    db_path: &Path,
) -> Result<Vec<SelectedPromptContextMemory>> {
    let reader = code_graph::open_code_graph_reader(Some(db_path)).with_context(|| {
        format!(
            "prompt memory code-context enrichment unavailable for {}",
            db_path.display()
        )
    })?;
    enrich_prompt_context_memories_with_reader(selected, reader.as_ref())
}

fn enrich_prompt_context_memories_with_code_context(
    selected: Vec<SelectedPromptContextMemory>,
) -> Result<Vec<SelectedPromptContextMemory>> {
    let db_path = resolve_memory_graph_db_path()?;
    enrich_prompt_context_memories_with_code_context_at_path(selected, &db_path)
}

fn retrieve_prompt_context_memories_from_backend(
    choice: BackendChoice,
    session_id: &str,
    query_text: &str,
    token_budget: usize,
) -> Result<Vec<PromptContextMemory>> {
    let memories = load_runtime_memories_from_backend(choice, session_id)?;
    let selected = select_prompt_context_memories(memories, query_text, token_budget);
    let selected = match choice {
        BackendChoice::GraphDb => enrich_prompt_context_memories_with_code_context(selected)?,
        BackendChoice::Sqlite => selected,
    };
    Ok(selected
        .into_iter()
        .map(|memory| PromptContextMemory {
            content: memory.content,
            code_context: memory.code_context,
        })
        .collect())
}

pub fn retrieve_prompt_context_memories(
    session_id: &str,
    query_text: &str,
    token_budget: usize,
) -> Result<Vec<PromptContextMemory>> {
    if session_id.trim().is_empty() || query_text.trim().is_empty() || token_budget == 0 {
        return Ok(Vec::new());
    }

    match resolve_memory_backend_preference() {
        Some(choice) => retrieve_prompt_context_memories_from_backend(
            choice,
            session_id,
            query_text,
            token_budget,
        ),
        None => retrieve_prompt_context_memories_from_backend(
            BackendChoice::GraphDb,
            session_id,
            query_text,
            token_budget,
        ),
    }
}

fn build_memory_id(record: &SessionLearningRecord, timestamp: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(record.session_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(record.agent_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(record.content.as_bytes());
    hasher.update(b"\0");
    hasher.update(timestamp.as_bytes());
    let digest = hasher.finalize();
    format!("mem-{:x}", digest)
}

fn heuristic_importance(content: &str) -> i64 {
    // Byte length as a proxy for character count — same result for ASCII,
    // slight overestimate for multibyte UTF-8, and O(1).
    let len = content.trim().len();
    match len {
        0..=99 => 5,
        100..=199 => 6,
        _ => 7,
    }
}

fn build_learning_record(
    session_id: &str,
    agent_id: &str,
    content: &str,
    task: Option<&str>,
    success: bool,
) -> Option<SessionLearningRecord> {
    let trimmed = content.trim();
    if trimmed.len() < 10 {
        return None;
    }

    let summary = trimmed.chars().take(500).collect::<String>();
    let title = summary.chars().take(50).collect::<String>();
    let project_id =
        std::env::var("AMPLIHACK_PROJECT_ID").unwrap_or_else(|_| "amplihack".to_string());
    Some(SessionLearningRecord {
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        content: format!("Agent {agent_id}: {summary}"),
        title: title.trim().to_string(),
        importance: heuristic_importance(trimmed),
        metadata: serde_json::json!({
            "new_memory_type": "semantic",
            "tags": ["learning", "session_end"],
            "task": task.unwrap_or_default(),
            "success": success,
            "project_id": project_id,
            "agent_type": agent_id,
        }),
    })
}

pub fn store_session_learning(
    session_id: &str,
    agent_id: &str,
    content: &str,
    task: Option<&str>,
    success: bool,
) -> Result<Option<String>> {
    let Some(record) = build_learning_record(session_id, agent_id, content, task, success) else {
        return Ok(None);
    };

    match resolve_memory_backend_preference() {
        Some(choice) => store_learning_with_backend(choice, &record),
        None => store_learning_with_backend(BackendChoice::GraphDb, &record),
    }
}

fn store_learning_with_backend(
    choice: BackendChoice,
    record: &SessionLearningRecord,
) -> Result<Option<String>> {
    self::backend::open_runtime_backend(choice)?.store_session_learning(record)
}

#[cfg(test)]
mod autodetect_test;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::home_env_lock;
    use rusqlite::{Connection as SqliteConnection, params};
    use std::fs;

    // -----------------------------------------------------------------------
    // SQLite tests (existing)
    // -----------------------------------------------------------------------

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

    #[test]
    fn retrieve_prompt_context_memories_reads_sqlite_backend() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let prev_home = std::env::var_os("HOME");
        let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("AMPLIHACK_MEMORY_BACKEND", "sqlite");
        }

        let conn = open_sqlite_memory_db()?;
        conn.execute(
            "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
            params!["prompt-session", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
        )?;
        conn.execute(
            "INSERT INTO session_agents (session_id, agent_id, first_used, last_used) VALUES (?1, ?2, ?3, ?4)",
            params![
                "prompt-session",
                "agent1",
                "2026-01-02T03:04:05",
                "2026-01-02T03:04:05"
            ],
        )?;
        conn.execute(
            "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                "m1",
                "prompt-session",
                "agent1",
                "learning",
                "Fix CI",
                "To fix CI, rerun cargo fmt and cargo clippy before pushing.",
                r#"{"new_memory_type":"semantic"}"#,
                8,
                "2026-01-02T03:04:05",
                "2099-01-02T03:04:05"
            ],
        )?;
        conn.execute(
            "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                "m2",
                "prompt-session",
                "agent1",
                "context",
                "Temporary note",
                "This is only temporary working memory.",
                r#"{"new_memory_type":"working"}"#,
                10,
                "2026-01-02T03:04:05",
                "2099-01-02T03:04:05"
            ],
        )?;

        let memories = retrieve_prompt_context_memories("prompt-session", "fix ci", 2000)?;

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match prev_backend {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
        }

        assert_eq!(memories.len(), 1);
        assert!(memories[0].content.contains("rerun cargo fmt"));
        assert_eq!(memories[0].code_context, None);
        Ok(())
    }

    #[test]
    fn retrieve_prompt_context_memories_enriches_graph_db_code_context() -> Result<()> {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let db_path = dir.path().join(".amplihack").join("graph_db");
        let prev_home = std::env::var_os("HOME");
        let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("AMPLIHACK_MEMORY_BACKEND", "graph-db");
            std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", &db_path);
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        }

        let record = SessionLearningRecord {
            session_id: "prompt-session".to_string(),
            agent_id: "agent1".to_string(),
            content: "Investigated helper behavior in src/example/module.py.".to_string(),
            title: "Helper behavior".to_string(),
            metadata: serde_json::json!({
                "new_memory_type": "semantic",
                "file": "src/example/module.py"
            }),
            importance: 8,
        };
        let memory_id = store_learning_with_backend(BackendChoice::GraphDb, &record)?
            .expect("memory should be stored");

        let json_path = dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::json!({
                "files": [
                    {"path":"src/example/module.py","language":"python","lines_of_code":10},
                    {"path":"src/example/utils.py","language":"python","lines_of_code":5}
                ],
                "classes": [
                    {"id":"class:Example","name":"Example","file_path":"src/example/module.py","line_number":1}
                ],
                "functions": [
                    {"id":"func:Example.process","name":"process","file_path":"src/example/module.py","line_number":2,"class_id":"class:Example"},
                    {"id":"func:helper","name":"helper","file_path":"src/example/utils.py","line_number":1,"signature":"def helper()","docstring":"Helper function"}
                ],
                "imports": [],
                "relationships": [
                    {"type":"CALLS","source_id":"func:Example.process","target_id":"func:helper"}
                ]
            })
            .to_string(),
        )?;
        super::code_graph::import_blarify_json(&json_path, Some(&db_path))?;

        let memories = retrieve_prompt_context_memories("prompt-session", "helper", 2000)?;

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match prev_backend {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
        }
        match prev_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(memories.len(), 1);
        assert!(memories[0].content.contains("Investigated helper behavior"));
        let code_context = memories[0]
            .code_context
            .as_deref()
            .expect("graph-db prompt memory should include code context");
        assert!(code_context.contains("**Related Files:**"));
        assert!(code_context.contains("src/example/module.py"));
        assert!(code_context.contains("**Related Functions:**"));
        assert!(code_context.contains("helper"));
        assert!(!memory_id.is_empty());
        Ok(())
    }

    #[test]
    fn retrieve_prompt_context_memories_does_not_silently_fallback_to_sqlite() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let prev_home = std::env::var_os("HOME");
        let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        let graph_parent_blocker = dir.path().join("graph-parent-blocker");
        fs::write(&graph_parent_blocker, "blocker")?;

        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::remove_var("AMPLIHACK_MEMORY_BACKEND");
            std::env::set_var(
                "AMPLIHACK_GRAPH_DB_PATH",
                graph_parent_blocker.join("graph_db"),
            );
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        }

        let conn = open_sqlite_memory_db()?;
        conn.execute(
            "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
            params!["prompt-session", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
        )?;
        conn.execute(
            "INSERT INTO session_agents (session_id, agent_id, first_used, last_used) VALUES (?1, ?2, ?3, ?4)",
            params!["prompt-session", "agent1", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
        )?;
        conn.execute(
            "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                "m1",
                "prompt-session",
                "agent1",
                "learning",
                "Fix CI",
                "SQLite memory should not be used when default Kuzu setup fails.",
                r#"{"new_memory_type":"semantic"}"#,
                8,
                "2026-01-02T03:04:05",
                "2099-01-02T03:04:05"
            ],
        )?;

        let result = retrieve_prompt_context_memories("prompt-session", "fix ci", 2000);

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match prev_backend {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
        }
        match prev_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let error =
            result.expect_err("default backend path should not silently fall back to sqlite");
        assert!(
            error.to_string().contains("No such file or directory")
                || error.to_string().contains("File exists")
                || error.to_string().contains("missing")
                || error.to_string().contains("failed"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn enrich_prompt_context_memories_with_code_context_surfaces_graph_open_failure() -> Result<()>
    {
        let dir = tempfile::tempdir()?;
        let blocker = dir.path().join("graph-parent-blocker");
        fs::write(&blocker, "blocker")?;
        let db_path = blocker.join("graph_db");

        let result = enrich_prompt_context_memories_with_code_context_at_path(
            vec![SelectedPromptContextMemory {
                memory_id: "mem-1".to_string(),
                content: "Investigated helper behavior.".to_string(),
                code_context: None,
            }],
            &db_path,
        );

        assert!(
            result.is_err(),
            "expected graph-open failure to surface, got Ok: {:?}",
            result.ok()
        );
        let error = result.err().unwrap().to_string();
        assert!(
            error.contains("prompt memory code-context enrichment unavailable")
                || error.contains("File exists")
                || error.contains("Not a directory")
                || error.contains("os error"),
            "expected explicit graph-open failure, got: {error}"
        );
        Ok(())
    }

    #[test]
    fn enrich_prompt_context_memories_with_code_context_surfaces_context_lookup_failure()
    -> Result<()> {
        struct FailingReader;

        impl code_graph::CodeGraphReaderBackend for FailingReader {
            fn stats(&self) -> Result<code_graph::CodeGraphStats> {
                Ok(code_graph::CodeGraphStats::default())
            }

            fn context_payload(
                &self,
                _memory_id: &str,
            ) -> Result<code_graph::CodeGraphContextPayload> {
                Err(anyhow::anyhow!("synthetic code-context failure"))
            }

            fn files(&self, _pattern: Option<&str>, _limit: u32) -> Result<Vec<String>> {
                Ok(Vec::new())
            }

            fn functions(
                &self,
                _file: Option<&str>,
                _limit: u32,
            ) -> Result<Vec<code_graph::CodeGraphNamedEntry>> {
                Ok(Vec::new())
            }

            fn classes(
                &self,
                _file: Option<&str>,
                _limit: u32,
            ) -> Result<Vec<code_graph::CodeGraphNamedEntry>> {
                Ok(Vec::new())
            }

            fn search(
                &self,
                _name: &str,
                _limit: u32,
            ) -> Result<Vec<code_graph::CodeGraphSearchEntry>> {
                Ok(Vec::new())
            }

            fn callers(
                &self,
                _name: &str,
                _limit: u32,
            ) -> Result<Vec<code_graph::CodeGraphEdgeEntry>> {
                Ok(Vec::new())
            }

            fn callees(
                &self,
                _name: &str,
                _limit: u32,
            ) -> Result<Vec<code_graph::CodeGraphEdgeEntry>> {
                Ok(Vec::new())
            }
        }

        let result = enrich_prompt_context_memories_with_reader(
            vec![SelectedPromptContextMemory {
                memory_id: "mem-lookup".to_string(),
                content: "Remember helper behavior.".to_string(),
                code_context: None,
            }],
            &FailingReader,
        );

        assert!(
            result.is_err(),
            "expected context lookup failure to surface, got Ok: {:?}",
            result.ok()
        );
        let error = result.err().unwrap();
        let error_message = error.to_string();
        let error_chain = format!("{error:#}");
        assert!(
            error_message.contains("failed to load prompt memory code context for mem-lookup"),
            "expected memory-specific lookup error, got: {error_message}"
        );
        assert!(
            error_chain.contains("synthetic code-context failure"),
            "expected root-cause context lookup error, got: {error_chain}"
        );
        Ok(())
    }

    #[test]
    fn resolve_memory_graph_db_path_prefers_env_override() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let override_path = dir.path().join("project-kuzu");
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", &override_path) };

        let resolved = resolve_memory_graph_db_path()?;

        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(resolved, override_path);
        Ok(())
    }

    #[test]
    fn store_session_learning_does_not_silently_fallback_to_sqlite() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let prev_home = std::env::var_os("HOME");
        let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        let graph_parent_blocker = dir.path().join("graph-parent-blocker");
        fs::write(&graph_parent_blocker, "blocker")?;

        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::remove_var("AMPLIHACK_MEMORY_BACKEND");
            std::env::set_var(
                "AMPLIHACK_GRAPH_DB_PATH",
                graph_parent_blocker.join("graph_db"),
            );
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        }

        let sqlite_path = dir.path().join(".amplihack").join("memory.db");
        if let Some(parent) = sqlite_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = open_sqlite_memory_db()?;
        conn.execute_batch(SQLITE_SCHEMA)?;

        let result = store_session_learning(
            "prompt-session",
            "agent1",
            "This learning record is long enough to persist if sqlite fallback were still active.",
            Some("prove no fallback"),
            true,
        );

        let sqlite_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM memory_entries", [], |row| row.get(0))?;

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match prev_backend {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
        }
        match prev_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(
            sqlite_count, 0,
            "sqlite fallback should not have stored anything"
        );
        let error =
            result.expect_err("default learning storage should not silently fall back to sqlite");
        assert!(
            error.to_string().contains("No such file or directory")
                || error.to_string().contains("File exists")
                || error.to_string().contains("missing")
                || error.to_string().contains("failed"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn resolve_memory_graph_db_path_prefers_backend_neutral_override() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let override_path = dir.path().join("project-graph");
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", &override_path) };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", dir.path().join("project-kuzu")) };

        let resolved = resolve_memory_graph_db_path()?;

        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(resolved, override_path);
        Ok(())
    }

    #[test]
    fn resolve_memory_graph_db_path_rejects_relative_graph_override() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        let prev_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", dir.path()) };
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "relative/graph.db") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

        let error = resolve_memory_graph_db_path().unwrap_err();

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let rendered = format!("{error:#}");
        assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
        assert!(rendered.contains("memory graph DB path must be absolute"));
        Ok(())
    }

    #[test]
    fn resolve_memory_graph_db_path_rejects_proc_prefixed_graph_override() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        let prev_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", dir.path()) };
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/proc/1/mem") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

        let error = resolve_memory_graph_db_path().unwrap_err();

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let rendered = format!("{error:#}");
        assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
        assert!(rendered.contains("blocked prefix /proc"));
        Ok(())
    }

    #[test]
    fn resolve_memory_graph_db_path_invalid_graph_override_does_not_fall_through_to_kuzu_alias()
    -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir()?;
        let kuzu_override = dir.path().join("project-kuzu");
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        let prev_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", dir.path()) };
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/tmp/../etc/shadow") };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", &kuzu_override) };

        let error = resolve_memory_graph_db_path().unwrap_err();

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let rendered = format!("{error:#}");
        assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
        assert!(rendered.contains("/tmp/../etc/shadow"));
        Ok(())
    }

    #[test]
    fn select_prompt_context_memories_respects_token_budget() {
        let memories = vec![
            MemoryRecord {
                memory_id: "m-large".to_string(),
                memory_type: "learning".to_string(),
                title: "Large".to_string(),
                content: "x".repeat(200),
                metadata: serde_json::json!({"new_memory_type": "semantic"}),
                importance: Some(10),
                accessed_at: Some("2099-01-02T03:04:05".to_string()),
                expires_at: None,
            },
            MemoryRecord {
                memory_id: "m-small".to_string(),
                memory_type: "learning".to_string(),
                title: "Small".to_string(),
                content: "fix ci quickly".to_string(),
                metadata: serde_json::json!({"new_memory_type": "semantic"}),
                importance: Some(1),
                accessed_at: Some("2099-01-02T03:04:05".to_string()),
                expires_at: None,
            },
        ];

        let selected = select_prompt_context_memories(memories, "fix ci", 10);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].memory_id, "m-small");
        assert_eq!(selected[0].content, "fix ci quickly");
    }

    #[test]
    fn build_learning_record_uses_semantic_metadata() {
        let record = build_learning_record(
            "sess-1",
            "analyzer",
            "Fixed CI by running cargo fmt and clippy locally before push.",
            Some("stabilize CI"),
            true,
        )
        .expect("record should be created");

        assert!(record.content.starts_with("Agent analyzer:"));
        assert_eq!(
            record
                .metadata
                .get("new_memory_type")
                .and_then(JsonValue::as_str),
            Some("semantic")
        );
        assert_eq!(
            record.metadata.get("task").and_then(JsonValue::as_str),
            Some("stabilize CI")
        );
    }

    // -----------------------------------------------------------------------
    // BackendChoice / TransferFormat unit tests
    // -----------------------------------------------------------------------

    /// BackendChoice::parse must accept "graph-db" and "sqlite", with "kuzu"
    /// retained as a compatibility alias.
    ///
    /// These tests are purely logic-level and do not touch the kuzu C++ FFI.
    /// They document the expected API contract for callers of the memory backend.
    #[test]
    fn backend_choice_parse_graph_db_and_kuzu_alias() {
        assert_eq!(
            BackendChoice::parse("graph-db").unwrap(),
            BackendChoice::GraphDb
        );
        assert_eq!(BackendChoice::parse("kuzu").unwrap(), BackendChoice::GraphDb);
    }

    #[test]
    fn backend_choice_parse_sqlite() {
        assert_eq!(
            BackendChoice::parse("sqlite").unwrap(),
            BackendChoice::Sqlite
        );
    }

    #[test]
    fn backend_choice_parse_invalid_returns_error() {
        assert!(
            BackendChoice::parse("postgres").is_err(),
            "Unknown backend names must be rejected"
        );
        assert!(
            BackendChoice::parse("").is_err(),
            "Empty string must be rejected"
        );
        assert!(
            BackendChoice::parse("KUZU").is_err(),
            "Case-sensitive: 'KUZU' is not 'kuzu'"
        );
    }

    #[test]
    fn transfer_format_parse_json() {
        assert_eq!(TransferFormat::parse("json").unwrap(), TransferFormat::Json);
    }

    #[test]
    fn transfer_format_parse_raw_db_and_kuzu_alias() {
        assert_eq!(
            TransferFormat::parse("raw-db").unwrap(),
            TransferFormat::RawDb
        );
        assert_eq!(
            TransferFormat::parse("kuzu").unwrap(),
            TransferFormat::RawDb
        );
    }

    #[test]
    fn transfer_format_parse_invalid_returns_error() {
        assert!(
            TransferFormat::parse("csv").is_err(),
            "Unsupported formats must be rejected"
        );
        assert!(
            TransferFormat::parse("").is_err(),
            "Empty string must be rejected"
        );
    }

    // -----------------------------------------------------------------------
    // parse_json_value unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_json_value_empty_string_returns_empty_object() {
        let val = parse_json_value("").unwrap();
        assert!(
            val.is_object(),
            "Empty string must parse to empty JSON object"
        );
        assert!(val.as_object().unwrap().is_empty());
    }

    #[test]
    fn parse_json_value_valid_json_parses_correctly() {
        let val = parse_json_value(r#"{"key": "value"}"#).unwrap();
        assert_eq!(val["key"], "value");
    }

    #[test]
    fn parse_json_value_invalid_json_returns_error() {
        assert!(
            parse_json_value("{not valid json}").is_err(),
            "Invalid JSON must return an error"
        );
    }
}
