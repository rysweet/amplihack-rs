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
mod sqlite_backend_atomicity_test;
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

pub(crate) struct HierarchicalImportPlan<'a> {
    pub(crate) episodic_nodes: Vec<&'a EpisodicNode>,
    pub(crate) semantic_nodes: Vec<&'a SemanticNode>,
    pub(crate) similar_to_edges: &'a [SimilarEdge],
    pub(crate) derives_from_edges: &'a [DerivesEdge],
    pub(crate) supersedes_edges: &'a [SupersedesEdge],
    pub(crate) transitioned_to_edges: &'a [TransitionEdge],
    pub(crate) stats: ImportStats,
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

pub(crate) fn build_hierarchical_import_plan<'a, F>(
    data: &'a HierarchicalExportData,
    merge: bool,
    mut has_existing_id: F,
) -> HierarchicalImportPlan<'a>
where
    F: FnMut(&str) -> bool,
{
    let mut stats = ImportStats::default();
    let mut episodic_nodes = Vec::new();
    for node in &data.episodic_nodes {
        if node.memory_id.is_empty() {
            stats.errors += 1;
            continue;
        }
        if merge && has_existing_id(&node.memory_id) {
            stats.skipped += 1;
            continue;
        }
        episodic_nodes.push(node);
    }

    let mut semantic_nodes = Vec::new();
    for node in &data.semantic_nodes {
        if node.memory_id.is_empty() {
            stats.errors += 1;
            continue;
        }
        if merge && has_existing_id(&node.memory_id) {
            stats.skipped += 1;
            continue;
        }
        semantic_nodes.push(node);
    }

    HierarchicalImportPlan {
        episodic_nodes,
        semantic_nodes,
        similar_to_edges: &data.similar_to_edges,
        derives_from_edges: &data.derives_from_edges,
        supersedes_edges: &data.supersedes_edges,
        transitioned_to_edges: &data.transitioned_to_edges,
        stats,
    }
}

pub(crate) fn build_hierarchical_import_result(
    agent_name: &str,
    source_agent: String,
    merge: bool,
    stats: ImportStats,
) -> ImportResult {
    ImportResult {
        agent_name: agent_name.to_string(),
        format: "json".to_string(),
        source_agent: Some(source_agent),
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
    match std::env::var("AMPLIHACK_MEMORY_BACKEND") {
        Ok(value) => match parse_backend_choice_env_value(&value) {
            Ok(choice) => choice,
            Err(err) => {
                tracing::warn!("{err}; defaulting to graph-db");
                BackendChoice::GraphDb
            }
        },
        Err(_) => BackendChoice::GraphDb,
    }
}

struct ResolvedTransferCliPolicy {
    choice: BackendChoice,
    format: TransferFormat,
    format_notice: Option<String>,
    storage_notice: Option<String>,
}

fn resolve_transfer_cli_policy(
    agent_name: &str,
    format: &str,
    storage_path: Option<&str>,
) -> Result<ResolvedTransferCliPolicy> {
    let choice = resolve_transfer_backend_choice();
    Ok(ResolvedTransferCliPolicy {
        choice,
        format: TransferFormat::parse(format)?,
        format_notice: transfer_format_cli_compatibility_notice(format),
        storage_notice: hierarchical_storage_compatibility_notice(
            agent_name,
            storage_path,
            choice,
        )?,
    })
}

pub fn run_export(
    agent_name: &str,
    output: &str,
    format: &str,
    storage_path: Option<&str>,
) -> Result<()> {
    let resolved = resolve_transfer_cli_policy(agent_name, format, storage_path)?;
    if let Some(notice) = resolved.format_notice.as_deref() {
        eprintln!("⚠️ Compatibility mode: {notice}");
    }
    if let Some(notice) = resolved.storage_notice.as_deref() {
        eprintln!("⚠️ Compatibility mode: {notice}");
    }
    match export_memory(
        agent_name,
        output,
        resolved.format,
        storage_path,
        resolved.choice,
    ) {
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
    let resolved = resolve_transfer_cli_policy(agent_name, format, storage_path)?;
    if let Some(notice) = resolved.format_notice.as_deref() {
        eprintln!("⚠️ Compatibility mode: {notice}");
    }
    if let Some(notice) = resolved.storage_notice.as_deref() {
        eprintln!("⚠️ Compatibility mode: {notice}");
    }
    match import_memory(
        agent_name,
        input,
        resolved.format,
        merge,
        storage_path,
        resolved.choice,
    ) {
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

struct HierarchicalMemoryPaths {
    graph_base: PathBuf,
    sqlite_db: PathBuf,
}

impl HierarchicalMemoryPaths {
    fn neutral_graph_db(&self) -> PathBuf {
        self.graph_base.join("graph_db")
    }

    fn legacy_graph_db(&self) -> PathBuf {
        self.graph_base.join("kuzu_db")
    }

    fn resolved_graph_db(&self) -> PathBuf {
        if matches!(
            self.graph_base.file_name().and_then(|name| name.to_str()),
            Some("graph_db" | "kuzu_db")
        ) || self.graph_base.join("kuzu.lock").exists()
        {
            return self.graph_base.clone();
        }

        if self.graph_base.is_dir() && !self.graph_base.join("kuzu.lock").exists() {
            let neutral = self.neutral_graph_db();
            let legacy = self.legacy_graph_db();
            // Prefer legacy kuzu_db when it exists AND neutral is absent or empty.
            // An auto-created empty graph_db directory (from a prior failed resolve) must
            // not override a populated kuzu_db — that is the regression this condition fixes.
            if legacy.exists() && (!neutral.exists() || is_dir_empty(&neutral)) {
                return legacy;
            }
            return neutral;
        }

        let neutral = self.neutral_graph_db();
        if neutral.exists() {
            return neutral;
        }

        let legacy = self.legacy_graph_db();
        if legacy.exists() {
            return legacy;
        }

        self.graph_base.clone()
    }
}

/// Returns `true` if `path` is a directory that contains no entries.
/// Returns `false` for non-existent paths, files, or directories with any content.
fn is_dir_empty(path: &Path) -> bool {
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

fn resolve_hierarchical_memory_paths(
    agent_name: &str,
    storage_path: Option<&str>,
) -> Result<HierarchicalMemoryPaths> {
    sqlite_backend::validate_agent_name(agent_name)?;
    let storage_root = match storage_path {
        Some(path) => PathBuf::from(path),
        None => memory_home_paths()?.hierarchical_memory_dir,
    };
    let graph_base = match storage_path {
        Some(_) => storage_root.clone(),
        None => storage_root.join(agent_name),
    };
    let sqlite_db = storage_root.join(format!("{agent_name}.db"));
    Ok(HierarchicalMemoryPaths {
        graph_base,
        sqlite_db,
    })
}

fn resolve_hierarchical_db_path(agent_name: &str, storage_path: Option<&str>) -> Result<PathBuf> {
    Ok(resolve_hierarchical_memory_paths(agent_name, storage_path)?.resolved_graph_db())
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
            ensure_parent_dir(&to)?;
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
    now.to_string()
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
    use crate::test_support::{home_env_lock, restore_home, set_home};
    use std::collections::HashSet;
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

    #[test]
    fn resolve_hierarchical_db_path_prefers_existing_legacy_store_path() -> Result<()> {
        let temp = tempdir()?;
        let agent_root = temp.path().join("agent-root");
        let legacy = agent_root.join("kuzu_db");
        fs::create_dir_all(&agent_root)?;
        fs::write(&legacy, b"legacy graph store placeholder")?;

        let resolved = resolve_hierarchical_db_path(
            "agent-b",
            Some(agent_root.to_str().expect("temp path should be utf-8")),
        )?;

        assert_eq!(resolved, legacy);
        Ok(())
    }

    #[test]
    fn resolve_hierarchical_db_path_keeps_direct_legacy_store_path() -> Result<()> {
        let temp = tempdir()?;
        let legacy = temp.path().join("kuzu_db");

        let resolved =
            resolve_hierarchical_db_path("agent-direct", Some(legacy.to_str().unwrap()))?;

        assert_eq!(resolved, legacy);
        Ok(())
    }

    // ----- Regression tests for empty-graph_db override bug -----

    /// Regression: populated kuzu_db + no graph_db sibling → resolver must return kuzu_db.
    /// This documents the expected behaviour for the parity-test happy path where graph_db
    /// has never been auto-created.
    #[test]
    fn resolve_hierarchical_db_path_populated_kuzu_db_no_graph_db_returns_kuzu_db() -> Result<()> {
        let temp = tempdir()?;
        let agent_root = temp.path().join("agent-root");
        let kuzu_db = agent_root.join("kuzu_db");
        fs::create_dir_all(&kuzu_db)?;
        // Simulate a populated kuzu database directory with at least one file.
        fs::write(kuzu_db.join("data.kuzu"), b"placeholder")?;

        let resolved = resolve_hierarchical_db_path(
            "agent-c",
            Some(agent_root.to_str().expect("temp path should be utf-8")),
        )?;

        assert_eq!(resolved, kuzu_db);
        Ok(())
    }

    /// Regression: populated kuzu_db + EMPTY graph_db sibling → resolver must still return
    /// kuzu_db.  This is the exact failure mode: open_graph_db_at_path() auto-creates an empty
    /// graph_db directory on a prior mis-resolve, which then causes a second resolve to prefer
    /// the empty neutral directory over the populated legacy one.
    #[test]
    fn resolve_hierarchical_db_path_populated_kuzu_db_empty_graph_db_returns_kuzu_db() -> Result<()>
    {
        let temp = tempdir()?;
        let agent_root = temp.path().join("agent-root");
        let kuzu_db = agent_root.join("kuzu_db");
        let graph_db = agent_root.join("graph_db");

        // kuzu_db is populated (non-empty).
        fs::create_dir_all(&kuzu_db)?;
        fs::write(kuzu_db.join("data.kuzu"), b"placeholder")?;

        // graph_db exists but is empty — the auto-created directory that triggers the bug.
        fs::create_dir_all(&graph_db)?;
        assert!(graph_db.exists());
        assert!(
            is_dir_empty(&graph_db),
            "graph_db must be empty for this regression test"
        );

        let resolved = resolve_hierarchical_db_path(
            "agent-c",
            Some(agent_root.to_str().expect("temp path should be utf-8")),
        )?;

        assert_eq!(
            resolved, kuzu_db,
            "resolver must prefer populated kuzu_db over auto-created empty graph_db"
        );
        Ok(())
    }

    /// Full export round-trip: import into kuzu_db under agent-root, then export from
    /// agent-root with a pre-existing empty graph_db directory (regression scenario).
    /// The export must return non-empty JSON (>0 nodes) from the kuzu_db, not from the
    /// empty graph_db that was auto-created before the fix was applied.
    #[test]
    fn hierarchical_json_export_agent_root_kuzu_db_with_empty_graph_db_sibling() -> Result<()> {
        let temp = tempdir()?;
        let agent_root = temp.path().join("agent-root");
        let legacy_db = agent_root.join("kuzu_db");
        let neutral_db = agent_root.join("graph_db");
        let input_path = temp.path().join("regression-input.json");
        let output_path = temp.path().join("regression-output.json");

        fs::create_dir_all(&agent_root)?;

        let payload = HierarchicalExportData {
            agent_name: "source-agent".to_string(),
            exported_at: "1704067200".to_string(),
            format_version: "1.1".to_string(),
            semantic_nodes: vec![SemanticNode {
                memory_id: "semantic-regression-1".to_string(),
                concept: "security".to_string(),
                content: "Regression: kuzu_db selected over empty graph_db sibling.".to_string(),
                confidence: 0.9,
                source_id: "episode-regression-1".to_string(),
                tags: vec!["regression".to_string()],
                metadata: serde_json::json!({"kind": "regression"}),
                created_at: "1704067200".to_string(),
                entity_name: "amplihack".to_string(),
            }],
            episodic_nodes: vec![EpisodicNode {
                memory_id: "episode-regression-1".to_string(),
                content: "Imported into kuzu_db; graph_db sibling is empty.".to_string(),
                source_label: "session".to_string(),
                tags: vec!["regression".to_string()],
                metadata: serde_json::json!({"turn": 1}),
                created_at: "1704067200".to_string(),
            }],
            similar_to_edges: vec![],
            derives_from_edges: vec![DerivesEdge {
                source_id: "semantic-regression-1".to_string(),
                target_id: "episode-regression-1".to_string(),
                extraction_method: "unit-test".to_string(),
                confidence: 0.9,
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

        let legacy_db_str = legacy_db.to_string_lossy().into_owned();
        let input_path_str = input_path.to_string_lossy().into_owned();
        let agent_root_str = agent_root.to_string_lossy().into_owned();
        let output_path_str = output_path.to_string_lossy().into_owned();

        // Import directly into a kuzu_db path. The underlying Kuzu library materializes
        // the store; the important regression is that later export from the agent root
        // resolves back to this legacy path instead of an empty graph_db sibling.
        import_memory(
            "target-agent",
            &input_path_str,
            TransferFormat::Json,
            false,
            Some(&legacy_db_str),
            BackendChoice::GraphDb,
        )?;

        // Simulate the auto-creation race: create an empty graph_db sibling.
        fs::create_dir_all(&neutral_db)?;
        assert!(
            is_dir_empty(&neutral_db),
            "graph_db must be empty to reproduce the regression"
        );

        // Export from agent-root — must resolve to kuzu_db, not the empty graph_db.
        let export = export_memory(
            "target-agent",
            &output_path_str,
            TransferFormat::Json,
            Some(&agent_root_str),
            BackendChoice::GraphDb,
        )?;
        assert_eq!(export.format, "json");

        let exported: HierarchicalExportData =
            serde_json::from_str(&fs::read_to_string(output_path)?)?;
        assert_eq!(
            exported.semantic_nodes.len(),
            1,
            "export must return >0 nodes from kuzu_db, not empty result from graph_db"
        );
        assert_eq!(
            exported.episodic_nodes.len(),
            1,
            "export must return >0 episodic nodes from kuzu_db"
        );
        assert_eq!(
            exported.semantic_nodes[0].memory_id,
            "semantic-regression-1"
        );
        Ok(())
    }

    #[test]
    fn hierarchical_json_export_from_agent_root_uses_populated_legacy_store() -> Result<()> {
        let temp = tempdir()?;
        let agent_root = temp.path().join("agent-root");
        let legacy_db = agent_root.join("kuzu_db");
        let neutral_db = agent_root.join("graph_db");
        let input_path = temp.path().join("legacy-input.json");
        let output_path = temp.path().join("legacy-output.json");

        fs::create_dir_all(&agent_root)?;
        let payload = HierarchicalExportData {
            agent_name: "source-agent".to_string(),
            exported_at: "1704067200".to_string(),
            format_version: "1.1".to_string(),
            semantic_nodes: vec![SemanticNode {
                memory_id: "semantic-legacy-1".to_string(),
                concept: "auth".to_string(),
                content: "Legacy kuzu_db should be exported from the agent root.".to_string(),
                confidence: 0.95,
                source_id: "episode-legacy-1".to_string(),
                tags: vec!["security".to_string()],
                metadata: serde_json::json!({"kind": "fact"}),
                created_at: "1704067200".to_string(),
                entity_name: "JWT".to_string(),
            }],
            episodic_nodes: vec![EpisodicNode {
                memory_id: "episode-legacy-1".to_string(),
                content: "Imported into the legacy graph store.".to_string(),
                source_label: "session".to_string(),
                tags: vec!["session".to_string()],
                metadata: serde_json::json!({"turn": 1}),
                created_at: "1704067200".to_string(),
            }],
            similar_to_edges: vec![],
            derives_from_edges: vec![DerivesEdge {
                source_id: "semantic-legacy-1".to_string(),
                target_id: "episode-legacy-1".to_string(),
                extraction_method: "unit-test".to_string(),
                confidence: 0.8,
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

        let legacy_db_str = legacy_db.to_string_lossy().into_owned();
        let input_path_str = input_path.to_string_lossy().into_owned();
        let agent_root_str = agent_root.to_string_lossy().into_owned();
        let output_path_str = output_path.to_string_lossy().into_owned();

        import_memory(
            "target-agent",
            &input_path_str,
            TransferFormat::Json,
            false,
            Some(&legacy_db_str),
            BackendChoice::GraphDb,
        )?;

        assert!(
            legacy_db.exists(),
            "legacy kuzu_db should exist after import"
        );
        assert!(
            !neutral_db.exists(),
            "agent-root regression requires kuzu_db without graph_db"
        );

        let export = export_memory(
            "target-agent",
            &output_path_str,
            TransferFormat::Json,
            Some(&agent_root_str),
            BackendChoice::GraphDb,
        )?;
        assert_eq!(export.format, "json");
        assert!(
            !neutral_db.exists(),
            "export should not create an empty graph_db sibling when kuzu_db is populated"
        );

        let exported: HierarchicalExportData =
            serde_json::from_str(&fs::read_to_string(output_path)?)?;
        assert_eq!(exported.semantic_nodes.len(), 1);
        assert_eq!(exported.episodic_nodes.len(), 1);
        assert_eq!(exported.derives_from_edges.len(), 1);
        assert_eq!(exported.semantic_nodes[0].memory_id, "semantic-legacy-1");
        Ok(())
    }

    #[test]
    fn resolve_hierarchical_memory_paths_defaults_to_shared_home_root() -> Result<()> {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempdir()?;
        let previous_home = set_home(temp.path());

        let paths = resolve_hierarchical_memory_paths("agent-a", None)?;
        let expected_root = memory_home_paths()?.hierarchical_memory_dir;

        restore_home(previous_home);
        assert_eq!(paths.graph_base, expected_root.join("agent-a"));
        assert_eq!(paths.sqlite_db, expected_root.join("agent-a.db"));
        Ok(())
    }

    #[test]
    fn resolve_hierarchical_memory_paths_uses_storage_override_for_both_backends() -> Result<()> {
        let temp = tempdir()?;
        let override_root = temp.path().join("override");
        fs::create_dir_all(&override_root)?;
        let override_str = override_root.to_string_lossy().into_owned();

        let paths = resolve_hierarchical_memory_paths("agent-a", Some(&override_str))?;

        assert_eq!(paths.graph_base, override_root);
        assert_eq!(paths.sqlite_db, override_root.join("agent-a.db"));
        Ok(())
    }

    #[test]
    fn hierarchical_import_plan_applies_shared_validation_and_merge_policy() {
        let data = HierarchicalExportData {
            agent_name: "source-agent".to_string(),
            exported_at: "1704067200".to_string(),
            format_version: "1.1".to_string(),
            semantic_nodes: vec![
                SemanticNode {
                    memory_id: "semantic-keep".to_string(),
                    concept: "repo".to_string(),
                    content: "Keep this semantic node".to_string(),
                    confidence: 0.9,
                    source_id: "episode-keep".to_string(),
                    tags: vec![],
                    metadata: serde_json::json!({}),
                    created_at: "1704067200".to_string(),
                    entity_name: "amplihack".to_string(),
                },
                SemanticNode {
                    memory_id: "semantic-skip".to_string(),
                    concept: "repo".to_string(),
                    content: "Skip on merge".to_string(),
                    confidence: 0.8,
                    source_id: "episode-keep".to_string(),
                    tags: vec![],
                    metadata: serde_json::json!({}),
                    created_at: "1704067200".to_string(),
                    entity_name: "amplihack".to_string(),
                },
                SemanticNode {
                    memory_id: String::new(),
                    concept: "repo".to_string(),
                    content: "Invalid".to_string(),
                    confidence: 0.7,
                    source_id: "episode-keep".to_string(),
                    tags: vec![],
                    metadata: serde_json::json!({}),
                    created_at: "1704067200".to_string(),
                    entity_name: "amplihack".to_string(),
                },
            ],
            episodic_nodes: vec![
                EpisodicNode {
                    memory_id: "episode-keep".to_string(),
                    content: "Keep this episodic node".to_string(),
                    source_label: "session".to_string(),
                    tags: vec![],
                    metadata: serde_json::json!({}),
                    created_at: "1704067200".to_string(),
                },
                EpisodicNode {
                    memory_id: "episode-skip".to_string(),
                    content: "Skip on merge".to_string(),
                    source_label: "session".to_string(),
                    tags: vec![],
                    metadata: serde_json::json!({}),
                    created_at: "1704067200".to_string(),
                },
                EpisodicNode {
                    memory_id: String::new(),
                    content: "Invalid".to_string(),
                    source_label: "session".to_string(),
                    tags: vec![],
                    metadata: serde_json::json!({}),
                    created_at: "1704067200".to_string(),
                },
            ],
            similar_to_edges: vec![SimilarEdge {
                source_id: "semantic-keep".to_string(),
                target_id: "semantic-skip".to_string(),
                weight: 0.4,
                metadata: serde_json::json!({}),
            }],
            derives_from_edges: vec![DerivesEdge {
                source_id: "semantic-keep".to_string(),
                target_id: "episode-keep".to_string(),
                extraction_method: "unit-test".to_string(),
                confidence: 0.7,
            }],
            supersedes_edges: vec![SupersedesEdge {
                source_id: "semantic-keep".to_string(),
                target_id: "semantic-skip".to_string(),
                reason: "new".to_string(),
                temporal_delta: "1".to_string(),
            }],
            transitioned_to_edges: vec![TransitionEdge {
                source_id: "semantic-keep".to_string(),
                target_id: "semantic-skip".to_string(),
                from_value: "old".to_string(),
                to_value: "new".to_string(),
                turn: 2,
                transition_type: "status".to_string(),
            }],
            statistics: HierarchicalStats {
                semantic_node_count: 3,
                episodic_node_count: 3,
                similar_to_edge_count: 1,
                derives_from_edge_count: 1,
                supersedes_edge_count: 1,
                transitioned_to_edge_count: 1,
            },
        };

        let existing_ids: HashSet<String> = ["semantic-skip", "episode-skip"]
            .into_iter()
            .map(str::to_string)
            .collect();

        let plan = build_hierarchical_import_plan(&data, true, |memory_id| {
            existing_ids.contains(memory_id)
        });

        assert_eq!(plan.episodic_nodes.len(), 1);
        assert_eq!(plan.semantic_nodes.len(), 1);
        assert_eq!(plan.episodic_nodes[0].memory_id, "episode-keep");
        assert_eq!(plan.semantic_nodes[0].memory_id, "semantic-keep");
        assert_eq!(plan.similar_to_edges.len(), 1);
        assert_eq!(plan.derives_from_edges.len(), 1);
        assert_eq!(plan.supersedes_edges.len(), 1);
        assert_eq!(plan.transitioned_to_edges.len(), 1);
        assert_eq!(plan.stats.skipped, 2);
        assert_eq!(plan.stats.errors, 2);
        assert_eq!(plan.stats.episodic_nodes_imported, 0);
        assert_eq!(plan.stats.semantic_nodes_imported, 0);
        assert_eq!(plan.stats.edges_imported, 0);
    }
}
