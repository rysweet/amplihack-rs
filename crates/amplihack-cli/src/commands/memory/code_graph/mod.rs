//! Native blarify JSON → graph-db code-graph import.

// These imports are consumed by backend.rs via `use super::*;`.
#[allow(unused_imports)]
use anyhow::{Context, Result, bail};
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use std::fs;
#[allow(unused_imports)]
use std::path::{Path, PathBuf};

pub(crate) mod backend;
mod import;
mod paths;
mod scip;
#[cfg(test)]
mod tests;
mod types;
mod validation;

pub use import::{import_blarify_json, import_scip_file, run_index_code, summarize_code_graph};
pub use paths::{
    code_graph_compatibility_notice_for_project, default_code_graph_db_path_for_project,
    resolve_code_graph_db_path_for_project,
};
pub use types::{
    CodeGraphContextClass, CodeGraphContextFile, CodeGraphContextFunction, CodeGraphContextPayload,
    CodeGraphEdgeEntry, CodeGraphImportCounts, CodeGraphNamedEntry, CodeGraphSearchEntry,
    CodeGraphStats, CodeGraphSummary,
};

pub(crate) use import::open_code_graph_reader;
pub(crate) use types::CodeGraphReaderBackend;
pub(crate) use validation::enforce_db_permissions;
#[allow(unused_imports)]
pub(crate) use validation::validate_index_path;

// Re-exports for backend submodule (consumed via `use super::*;`).
#[allow(unused_imports)]
pub(self) use paths::default_code_graph_db_path;
#[allow(unused_imports)]
pub(self) use types::{
    BLARIFY_JSON_MAX_BYTES, BlarifyClass, BlarifyFile, BlarifyFunction, BlarifyImport,
    BlarifyOutput, BlarifyRelationship, CodeGraphWriterBackend,
};
#[allow(unused_imports)]
pub(self) use validation::validate_blarify_json_size;
