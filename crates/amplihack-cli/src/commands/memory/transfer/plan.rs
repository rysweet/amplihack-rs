use super::types::{HierarchicalExportData, HierarchicalImportPlan, ImportResult, ImportStats};
use crate::commands::memory::{BackendChoice, parse_backend_choice_env_value};

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
