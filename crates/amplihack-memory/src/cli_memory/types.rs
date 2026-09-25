use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::path::{Path, PathBuf};

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

pub(crate) fn backend_cli_compatibility_notice(backend: &str) -> Option<String> {
    (backend == "kuzu")
        .then(|| "CLI value `kuzu` is a legacy compatibility alias; prefer `graph-db`.".to_string())
}

pub(crate) fn transfer_format_cli_compatibility_notice(format: &str) -> Option<String> {
    (format == "kuzu")
        .then(|| "CLI value `kuzu` is a legacy compatibility alias; prefer `raw-db`.".to_string())
}

pub(crate) struct ResolvedMemoryCliBackend {
    pub(crate) choice: BackendChoice,
    pub(crate) cli_notice: Option<String>,
    pub(crate) graph_notice: Option<String>,
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
pub(super) struct SelectedPromptContextMemory {
    pub(super) memory_id: String,
    pub(super) content: String,
    pub(super) code_context: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionLearningRecord {
    pub(crate) session_id: String,
    pub(crate) agent_id: String,
    pub(crate) content: String,
    pub(crate) title: String,
    pub(crate) metadata: JsonValue,
    pub(crate) importance: i64,
}

pub(crate) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemoryHomePaths {
    pub(crate) root_dir: PathBuf,
    pub(crate) graph_db: PathBuf,
    pub(crate) legacy_graph_db: PathBuf,
    pub(crate) sqlite_db: PathBuf,
    pub(crate) hierarchical_memory_dir: PathBuf,
}

pub(crate) fn memory_home_paths() -> Result<MemoryHomePaths> {
    let root_dir = home_dir()?.join(".amplihack");
    Ok(MemoryHomePaths {
        graph_db: root_dir.join("memory_graph.db"),
        legacy_graph_db: root_dir.join("memory_kuzu.db"),
        sqlite_db: root_dir.join("memory.db"),
        hierarchical_memory_dir: root_dir.join("hierarchical_memory"),
        root_dir,
    })
}

/// Per-project artifact paths, all under a cache directory **outside** the
/// project checkout.
///
/// Deliberately **not** `#[non_exhaustive]`: every consumer is in this
/// workspace, and exhaustive destructuring is what makes the compiler point at
/// every call site when a field is added or removed. A missed consumer here is
/// an artifact written back into someone's repository, not a compile warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectArtifactPaths {
    pub artifact_dir: PathBuf,
    pub indexes_dir: PathBuf,
    pub blarify_json: PathBuf,
    pub index_scip: PathBuf,
    pub indexing_pid: PathBuf,
    pub blarify_stale: PathBuf,
}

/// Resolve the artifact paths for `project_path`.
///
/// Returns `Err` rather than falling back to a project-relative path:
/// returning one on failure would reintroduce issue #1476 on exactly the
/// systems where it is hardest to notice.
pub fn project_artifact_paths(project_path: &Path) -> Result<ProjectArtifactPaths> {
    let artifact_dir = super::artifact_root::project_artifact_root(project_path)?;
    Ok(ProjectArtifactPaths {
        indexes_dir: artifact_dir.join("indexes"),
        blarify_json: artifact_dir.join("blarify.json"),
        index_scip: artifact_dir.join("index.scip"),
        indexing_pid: artifact_dir.join("indexing.pid"),
        blarify_stale: artifact_dir.join("blarify_stale"),
        artifact_dir,
    })
}
