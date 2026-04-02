use super::super::*;
use crate::commands::memory::BackendChoice;
use anyhow::Result;

/// Maximum allowed JSON file size for graph-db imports: 500 MiB.
/// Mirrors the same guard in the SQLite backend.
pub(in crate::commands::memory::transfer) const MAX_JSON_FILE_SIZE: u64 = 500 * 1024 * 1024;

pub(in crate::commands::memory::transfer) trait HierarchicalTransferBackend {
    fn export_hierarchical_json(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult>;
    fn import_hierarchical_json(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult>;
    fn export_hierarchical_raw_db(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult>;
    fn import_hierarchical_raw_db(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult>;
}

pub(in crate::commands::memory::transfer) struct GraphDbHierarchicalTransferBackend;

pub(in crate::commands::memory::transfer) fn open_hierarchical_transfer_backend_for(
    choice: BackendChoice,
) -> Box<dyn HierarchicalTransferBackend> {
    match choice {
        BackendChoice::Sqlite => {
            Box::new(super::super::sqlite_backend::SqliteHierarchicalTransferBackend)
        }
        BackendChoice::GraphDb => Box::new(GraphDbHierarchicalTransferBackend),
    }
}

pub(in crate::commands::memory::transfer) fn export_hierarchical_json(
    agent_name: &str,
    output: &str,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ExportResult> {
    open_hierarchical_transfer_backend_for(choice).export_hierarchical_json(
        agent_name,
        output,
        storage_path,
    )
}

pub(in crate::commands::memory::transfer) fn import_hierarchical_json(
    agent_name: &str,
    input: &str,
    merge: bool,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ImportResult> {
    open_hierarchical_transfer_backend_for(choice).import_hierarchical_json(
        agent_name,
        input,
        merge,
        storage_path,
    )
}

pub(in crate::commands::memory::transfer) fn export_hierarchical_raw_db(
    agent_name: &str,
    output: &str,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ExportResult> {
    open_hierarchical_transfer_backend_for(choice).export_hierarchical_raw_db(
        agent_name,
        output,
        storage_path,
    )
}

pub(in crate::commands::memory::transfer) fn import_hierarchical_raw_db(
    agent_name: &str,
    input: &str,
    merge: bool,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ImportResult> {
    open_hierarchical_transfer_backend_for(choice).import_hierarchical_raw_db(
        agent_name,
        input,
        merge,
        storage_path,
    )
}

impl HierarchicalTransferBackend for GraphDbHierarchicalTransferBackend {
    fn export_hierarchical_json(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult> {
        super::graph_export::export_hierarchical_json_impl(agent_name, output, storage_path)
    }

    fn import_hierarchical_json(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult> {
        super::graph_import::import_hierarchical_json_impl(agent_name, input, merge, storage_path)
    }

    fn export_hierarchical_raw_db(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult> {
        super::graph_export::export_hierarchical_raw_db_impl(agent_name, output, storage_path)
    }

    fn import_hierarchical_raw_db(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult> {
        super::graph_import::import_hierarchical_raw_db_impl(agent_name, input, merge, storage_path)
    }
}
