//! `memory export` and `memory import` command implementations.

use super::*;
use crate::command_error::exit_error;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;

pub(crate) mod backend;
pub(crate) mod sqlite_backend;

#[cfg(test)]
mod backend_choice_test;
#[cfg(test)]
mod sqlite_backend_security_test;
#[cfg(test)]
mod sqlite_schema_test;
#[cfg(test)]
mod transfer_backend_parity_test;

/// Maximum directory depth to prevent unbounded recursion.
const MAX_DIR_DEPTH: usize = 64;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HierarchicalExportData {
    pub(crate) agent_name: String,
    pub(crate) exported_at: String,
    pub(crate) format_version: String,
    pub(crate) semantic_nodes: Vec<SemanticNode>,
    pub(crate) episodic_nodes: Vec<EpisodicNode>,
    pub(crate) similar_to_edges: Vec<SimilarEdge>,
    pub(crate) derives_from_edges: Vec<DerivesEdge>,
    pub(crate) supersedes_edges: Vec<SupersedesEdge>,
    pub(crate) transitioned_to_edges: Vec<TransitionEdge>,
    #[serde(default)]
    pub(crate) statistics: HierarchicalStats,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SemanticNode {
    pub(crate) memory_id: String,
    pub(crate) concept: String,
    pub(crate) content: String,
    pub(crate) confidence: f64,
    pub(crate) source_id: String,
    pub(crate) tags: Vec<String>,
    pub(crate) metadata: JsonValue,
    pub(crate) created_at: String,
    pub(crate) entity_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct EpisodicNode {
    pub(crate) memory_id: String,
    pub(crate) content: String,
    pub(crate) source_label: String,
    pub(crate) tags: Vec<String>,
    pub(crate) metadata: JsonValue,
    pub(crate) created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SimilarEdge {
    pub(crate) source_id: String,
    pub(crate) target_id: String,
    pub(crate) weight: f64,
    pub(crate) metadata: JsonValue,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DerivesEdge {
    pub(crate) source_id: String,
    pub(crate) target_id: String,
    pub(crate) extraction_method: String,
    pub(crate) confidence: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SupersedesEdge {
    pub(crate) source_id: String,
    pub(crate) target_id: String,
    pub(crate) reason: String,
    pub(crate) temporal_delta: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TransitionEdge {
    pub(crate) source_id: String,
    pub(crate) target_id: String,
    pub(crate) from_value: String,
    pub(crate) to_value: String,
    pub(crate) turn: i64,
    pub(crate) transition_type: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct HierarchicalStats {
    pub(crate) semantic_node_count: usize,
    pub(crate) episodic_node_count: usize,
    pub(crate) similar_to_edge_count: usize,
    pub(crate) derives_from_edge_count: usize,
    pub(crate) supersedes_edge_count: usize,
    pub(crate) transitioned_to_edge_count: usize,
}

#[derive(Debug, Default)]
pub(crate) struct ImportStats {
    pub(crate) semantic_nodes_imported: usize,
    pub(crate) episodic_nodes_imported: usize,
    pub(crate) edges_imported: usize,
    pub(crate) skipped: usize,
    pub(crate) errors: usize,
}

#[derive(Debug)]
pub(crate) struct ExportResult {
    pub(crate) agent_name: String,
    pub(crate) format: String,
    pub(crate) output_path: String,
    pub(crate) file_size_bytes: Option<u64>,
    pub(crate) statistics: Vec<(String, String)>,
}

impl ExportResult {
    pub(crate) fn statistics_lines(&self) -> Vec<(String, String)> {
        self.statistics.clone()
    }
}

#[derive(Debug)]
pub(crate) struct ImportResult {
    pub(crate) agent_name: String,
    pub(crate) format: String,
    pub(crate) source_agent: Option<String>,
    pub(crate) merge: bool,
    pub(crate) statistics: Vec<(String, String)>,
}

impl ImportResult {
    pub(crate) fn statistics_lines(&self) -> Vec<(String, String)> {
        self.statistics.clone()
    }
}

/// Resolve the backend choice for transfer operations from the environment.
///
/// Resolution order:
/// - `AMPLIHACK_MEMORY_BACKEND=sqlite` → `BackendChoice::Sqlite`
/// - `AMPLIHACK_MEMORY_BACKEND=kuzu` → `BackendChoice::GraphDb`
/// - `AMPLIHACK_MEMORY_BACKEND=graph-db` → `BackendChoice::GraphDb` (alias)
/// - Unrecognized value → warn and default to the `graph-db` backend
/// - Not set → default to the `graph-db` backend
pub(crate) fn resolve_transfer_backend_choice() -> BackendChoice {
    match std::env::var("AMPLIHACK_MEMORY_BACKEND").ok().as_deref() {
        Some("sqlite") => BackendChoice::Sqlite,
        Some("kuzu") | Some("graph-db") => BackendChoice::GraphDb,
        Some(other) => {
            tracing::warn!(
                "Unrecognized AMPLIHACK_MEMORY_BACKEND value {other:?}; defaulting to graph-db"
            );
            BackendChoice::GraphDb
        }
        None => BackendChoice::GraphDb,
    }
}

pub fn run_export(
    agent_name: &str,
    output: &str,
    format: &str,
    storage_path: Option<&str>,
) -> Result<()> {
    let choice = resolve_transfer_backend_choice();
    if let Some(notice) =
        hierarchical_storage_compatibility_notice(agent_name, storage_path, choice)?
    {
        println!("⚠️ Compatibility mode: {notice}");
    }
    let format = TransferFormat::parse(format);
    match format.and_then(|fmt| export_memory(agent_name, output, fmt, storage_path, choice)) {
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
    let choice = resolve_transfer_backend_choice();
    if let Some(notice) =
        hierarchical_storage_compatibility_notice(agent_name, storage_path, choice)?
    {
        println!("⚠️ Compatibility mode: {notice}");
    }
    let format = TransferFormat::parse(format);
    match format.and_then(|fmt| import_memory(agent_name, input, fmt, merge, storage_path, choice))
    {
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

fn hierarchical_storage_compatibility_notice(
    agent_name: &str,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<Option<String>> {
    if !matches!(choice, BackendChoice::GraphDb) {
        return Ok(None);
    }

    let resolved = resolve_hierarchical_db_path(agent_name, storage_path)?;
    if resolved.file_name().and_then(|name| name.to_str()) != Some("kuzu_db") {
        return Ok(None);
    }

    let neutral = resolved.with_file_name("graph_db");
    Ok(Some(format!(
        "using legacy hierarchical store `{}` because `{}` is not active; migrate to `graph_db`.",
        resolved.display(),
        neutral.display()
    )))
}

fn export_memory(
    agent_name: &str,
    output: &str,
    format: TransferFormat,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ExportResult> {
    match format {
        TransferFormat::Json => {
            backend::export_hierarchical_json(agent_name, output, storage_path, choice)
        }
        TransferFormat::RawDb => {
            backend::export_hierarchical_raw_db(agent_name, output, storage_path, choice)
        }
    }
}

fn import_memory(
    agent_name: &str,
    input: &str,
    format: TransferFormat,
    merge: bool,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ImportResult> {
    match format {
        TransferFormat::Json => {
            backend::import_hierarchical_json(agent_name, input, merge, storage_path, choice)
        }
        TransferFormat::RawDb => {
            backend::import_hierarchical_raw_db(agent_name, input, merge, storage_path, choice)
        }
    }
}

fn resolve_hierarchical_db_path(agent_name: &str, storage_path: Option<&str>) -> Result<PathBuf> {
    // Validate agent name to prevent path traversal (e.g. "../evil") in the
    // Kuzu/graph-db backend, mirroring the check in resolve_hierarchical_sqlite_path.
    sqlite_backend::validate_agent_name(agent_name)?;
    let base = match storage_path {
        Some(path) => PathBuf::from(path),
        None => home_dir()?
            .join(".amplihack")
            .join("hierarchical_memory")
            .join(agent_name),
    };
    if base.is_dir() && !base.join("kuzu.lock").exists() {
        let neutral = base.join("graph_db");
        let legacy = base.join("kuzu_db");
        if legacy.is_dir() && !neutral.exists() {
            return Ok(legacy);
        }
        return Ok(neutral);
    }
    if base.join("graph_db").is_dir() {
        return Ok(base.join("graph_db"));
    }
    if base.join("kuzu_db").is_dir() {
        return Ok(base.join("kuzu_db"));
    }
    Ok(base)
}

fn copy_hierarchical_storage(src: &Path, dst: &Path) -> Result<()> {
    use anyhow::Context;
    if src.is_dir() {
        copy_dir_recursive(src, dst)?;
        return Ok(());
    }
    fs::copy(src, dst)
        .with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    let mut seen = HashSet::new();
    copy_dir_recursive_inner(src, dst, 0, &mut seen)
}

fn copy_dir_recursive_inner(
    src: &Path,
    dst: &Path,
    depth: usize,
    seen: &mut HashSet<PathBuf>,
) -> Result<()> {
    if depth > MAX_DIR_DEPTH {
        anyhow::bail!(
            "copy_dir_recursive exceeded maximum depth ({MAX_DIR_DEPTH}) at {}",
            src.display()
        );
    }
    fs::create_dir_all(dst)?;
    let canonical = src.canonicalize().unwrap_or_else(|_| src.to_path_buf());
    if !seen.insert(canonical) {
        anyhow::bail!("symlink cycle detected at {}", src.display());
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if kind.is_symlink() {
            // Skip symlinks with a warning to prevent directory traversal attacks
            println!("  Skipping symlink: {}", from.display());
            continue;
        } else if kind.is_dir() {
            copy_dir_recursive_inner(&from, &to, depth + 1, seen)?;
        } else if kind.is_file() {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn compute_path_size(path: &Path) -> Result<u64> {
    compute_path_size_inner(path, 0)
}

fn compute_path_size_inner(path: &Path, depth: usize) -> Result<u64> {
    if depth > MAX_DIR_DEPTH {
        anyhow::bail!(
            "compute_path_size exceeded maximum depth ({MAX_DIR_DEPTH}) at {}",
            path.display()
        );
    }
    if path.is_file() {
        return Ok(path.metadata()?.len());
    }
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        total += compute_path_size_inner(&entry.path(), depth + 1)?;
    }
    Ok(total)
}

pub(crate) fn graph_export_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Match Python well enough for parity comparisons that normalize timestamps.
    format!("{}", now)
}

pub(crate) fn parse_json_array_of_strings(value: &str) -> Result<Vec<String>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn hierarchical_json_round_trip_uses_public_transfer_flow() -> Result<()> {
        let temp = tempdir()?;
        let db_path = temp.path().join("roundtrip.graph_db");
        let input_path = temp.path().join("input.json");
        let output_path = temp.path().join("output.json");

        let payload = HierarchicalExportData {
            agent_name: "source-agent".to_string(),
            exported_at: "1704067200".to_string(),
            format_version: "1.1".to_string(),
            semantic_nodes: vec![SemanticNode {
                memory_id: "semantic-1".to_string(),
                concept: "repo".to_string(),
                content: "Remember the repo root".to_string(),
                confidence: 0.9,
                source_id: "episode-1".to_string(),
                tags: vec!["memory".to_string(), "repo".to_string()],
                metadata: serde_json::json!({"file": "README.md"}),
                created_at: "1704067200".to_string(),
                entity_name: "amplihack".to_string(),
            }],
            episodic_nodes: vec![EpisodicNode {
                memory_id: "episode-1".to_string(),
                content: "Opened the repository".to_string(),
                source_label: "session".to_string(),
                tags: vec!["session".to_string()],
                metadata: serde_json::json!({"turn": 1}),
                created_at: "1704067200".to_string(),
            }],
            similar_to_edges: vec![],
            derives_from_edges: vec![DerivesEdge {
                source_id: "semantic-1".to_string(),
                target_id: "episode-1".to_string(),
                extraction_method: "unit-test".to_string(),
                confidence: 0.75,
            }],
            supersedes_edges: vec![],
            transitioned_to_edges: vec![],
            statistics: HierarchicalStats {
                semantic_node_count: 1,
                episodic_node_count: 1,
                similar_to_edge_count: 0,
                derives_from_edge_count: 1,
                supersedes_edge_count: 0,
                transitioned_to_edge_count: 0,
            },
        };
        fs::write(&input_path, serde_json::to_string_pretty(&payload)?)?;

        let db_path_str = db_path.to_string_lossy().into_owned();
        let input_path_str = input_path.to_string_lossy().into_owned();
        let output_path_str = output_path.to_string_lossy().into_owned();

        let import = import_memory(
            "target-agent",
            &input_path_str,
            TransferFormat::Json,
            false,
            Some(&db_path_str),
            BackendChoice::GraphDb,
        )?;
        assert_eq!(import.source_agent.as_deref(), Some("source-agent"));
        assert!(
            import
                .statistics
                .iter()
                .any(|(name, value)| name == "semantic_nodes_imported" && value == "1")
        );

        let export = export_memory(
            "target-agent",
            &output_path_str,
            TransferFormat::Json,
            Some(&db_path_str),
            BackendChoice::GraphDb,
        )?;
        assert_eq!(export.format, "json");

        let exported: HierarchicalExportData =
            serde_json::from_str(&fs::read_to_string(output_path)?)?;
        assert_eq!(exported.agent_name, "target-agent");
        assert_eq!(exported.semantic_nodes.len(), 1);
        assert_eq!(exported.episodic_nodes.len(), 1);
        assert_eq!(exported.derives_from_edges.len(), 1);
        assert_eq!(exported.semantic_nodes[0].memory_id, "semantic-1");
        assert_eq!(exported.episodic_nodes[0].memory_id, "episode-1");

        Ok(())
    }

    #[test]
    fn resolve_hierarchical_db_path_prefers_neutral_subdir_for_agent_root() -> Result<()> {
        let temp = tempdir()?;
        let agent_root = temp.path().join("agent-root");
        fs::create_dir_all(&agent_root)?;

        let resolved = resolve_hierarchical_db_path(
            "agent-a",
            Some(agent_root.to_str().expect("temp path should be utf-8")),
        )?;

        assert_eq!(resolved, agent_root.join("graph_db"));
        Ok(())
    }

    #[test]
    fn resolve_hierarchical_db_path_prefers_existing_legacy_subdir() -> Result<()> {
        let temp = tempdir()?;
        let agent_root = temp.path().join("agent-root");
        let legacy = agent_root.join("kuzu_db");
        fs::create_dir_all(&legacy)?;

        let resolved = resolve_hierarchical_db_path(
            "agent-b",
            Some(agent_root.to_str().expect("temp path should be utf-8")),
        )?;

        assert_eq!(resolved, legacy);
        Ok(())
    }
}
