//! Native memory commands (`tree`, `export`, `import`, `clean`).

pub mod backend;
pub mod clean;
pub mod code_graph;
pub mod indexing_job;
pub mod scip_indexing;
pub mod staleness_detector;
pub mod time_estimator;
pub mod transfer;
pub mod tree;

mod helpers;
mod learning;
mod prompt_context;
mod resolve;
mod schema;
mod types;

// --- Re-exports from existing submodules (unchanged public API) ---

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
pub use time_estimator::{IndexTimeEstimate, estimate_indexing_time};
pub use transfer::{run_export, run_import};
pub use tree::run_tree;

// --- Re-exports from new submodules ---

pub use learning::{retrieve_prompt_context_memories, store_session_learning};
pub use types::{PromptContextMemory, SessionSummary};

pub(crate) use helpers::{
    ensure_parent_dir, parse_backend_choice_env_value, parse_json_value, required_parent_dir,
};
pub(crate) use prompt_context::parse_memory_timestamp;
pub(crate) use resolve::resolve_memory_cli_backend;
pub(crate) use schema::{GRAPH_DB_TREE_BACKEND_NAME, HIERARCHICAL_SCHEMA};
pub(crate) use types::{
    BackendChoice, MemoryRecord, SessionLearningRecord, TransferFormat,
    memory_home_paths, project_artifact_paths, transfer_format_cli_compatibility_notice,
};

// Private imports: accessible to descendant modules (tests) via `use super::*`.
use serde_json::Value as JsonValue;
#[cfg(test)]
use anyhow::Result;
#[cfg(test)]
use self::backend::graph_db::resolve_memory_graph_db_path;
#[cfg(test)]
use learning::{build_learning_record, store_learning_with_backend};
#[cfg(test)]
use prompt_context::{
    enrich_prompt_context_memories_with_code_context_at_path,
    enrich_prompt_context_memories_with_reader, select_prompt_context_memories,
};
#[cfg(test)]
use resolve::{
    memory_graph_compatibility_notice, resolve_backend_with_autodetect,
    resolve_memory_backend_preference,
};
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use types::{SelectedPromptContextMemory, backend_cli_compatibility_notice};

#[cfg(test)]
mod autodetect_test;

#[cfg(test)]
mod tests;
