//! `memory export` and `memory import` command implementations.

pub(crate) mod backend;
pub(crate) mod sqlite_backend;

mod commands;
mod paths;
mod plan;
mod types;

#[cfg(test)]
mod backend_choice_test;
#[cfg(test)]
mod sqlite_backend_atomicity_test;
#[cfg(test)]
mod sqlite_backend_security_test;
#[cfg(test)]
mod sqlite_schema_test;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transfer_backend_parity_test;

// --- Re-exports: types (pub(crate), used by backends and external test modules) ---
pub(crate) use types::{
    DerivesEdge, EpisodicNode, ExportResult, HierarchicalExportData, HierarchicalStats,
    ImportResult, ImportStats, SemanticNode, SimilarEdge, SupersedesEdge, TransitionEdge,
};

// --- Re-exports: plan functions (pub(crate)) ---
pub(crate) use plan::{build_hierarchical_import_plan, build_hierarchical_import_result};
// resolve_transfer_backend_choice is only accessed via absolute path from test modules.
#[cfg(test)]
pub(crate) use plan::resolve_transfer_backend_choice;

// --- Re-exports: path utilities (pub(crate)) ---
pub(crate) use paths::{graph_export_timestamp, parse_json_array_of_strings};

// --- Re-exports: CLI entry points (pub, re-exported by memory/mod.rs) ---
pub use commands::{run_export, run_import};

// Items needed by child modules (backend.rs, sqlite_backend.rs, tests) via `use super::*;`.
#[cfg(test)]
pub(crate) use commands::{export_memory, import_memory};
#[cfg(test)]
pub(crate) use paths::is_dir_empty;
pub(crate) use paths::{
    compute_path_size, copy_hierarchical_storage, resolve_hierarchical_db_path,
    resolve_hierarchical_memory_paths,
};
