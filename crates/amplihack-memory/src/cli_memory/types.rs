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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectArtifactPaths {
    pub(crate) artifact_dir: PathBuf,
    pub(crate) indexes_dir: PathBuf,
    pub(crate) blarify_json: PathBuf,
    pub(crate) root_index_scip: PathBuf,
    pub(crate) index_scip: PathBuf,
    pub(crate) index_scip_backup: PathBuf,
    pub(crate) indexing_pid: PathBuf,
}

pub(crate) fn project_artifact_paths(project_path: &Path) -> ProjectArtifactPaths {
    let artifact_dir = project_path.join(".amplihack");
    ProjectArtifactPaths {
        indexes_dir: artifact_dir.join("indexes"),
        blarify_json: artifact_dir.join("blarify.json"),
        root_index_scip: project_path.join("index.scip"),
        index_scip: artifact_dir.join("index.scip"),
        index_scip_backup: artifact_dir.join("index.scip.backup"),
        indexing_pid: artifact_dir.join("indexing.pid"),
        artifact_dir,
    }
}
