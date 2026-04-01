use super::super::super::backend::graph_db::GraphDbConnection;
use super::super::{
    BlarifyOutput, CodeGraphImportCounts, CodeGraphWriterBackend,
};
use super::import_ops::{import_classes, import_files, import_functions, import_imports};
use super::memory_links::link_memories_to_code_files_in_conn;
use super::relationships::import_relationships;
use super::{ensure_memory_code_link_schema, open_graph_db_code_graph_db};
use anyhow::Result;
use std::path::Path;

pub(in crate::commands::memory::code_graph) fn open_code_graph_writer(
    path_override: Option<&Path>,
) -> Result<Box<dyn CodeGraphWriterBackend>> {
    Ok(Box::new(GraphDbCodeGraphWriter::open(path_override)?))
}

struct GraphDbCodeGraphWriter {
    handle: super::super::super::backend::graph_db::GraphDbHandle,
}

impl GraphDbCodeGraphWriter {
    fn open(path_override: Option<&Path>) -> Result<Self> {
        Ok(Self {
            handle: open_graph_db_code_graph_db(path_override)?,
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&GraphDbConnection<'_>) -> Result<T>) -> Result<T> {
        self.handle
            .with_initialized_conn(ensure_memory_code_link_schema, f)
    }
}

impl CodeGraphWriterBackend for GraphDbCodeGraphWriter {
    fn import_blarify_output(&self, payload: &BlarifyOutput) -> Result<CodeGraphImportCounts> {
        self.with_conn(|conn| {
            let counts = CodeGraphImportCounts {
                files: import_files(conn, &payload.files)?,
                classes: import_classes(conn, &payload.classes)?,
                functions: import_functions(conn, &payload.functions)?,
                imports: import_imports(conn, &payload.imports)?,
                relationships: import_relationships(conn, &payload.relationships)?,
            };
            let linked_memories = link_memories_to_code_files_in_conn(conn)?;
            if linked_memories > 0 {
                tracing::info!(
                    count = linked_memories,
                    "linked existing memories to code graph after import"
                );
            }
            Ok(counts)
        })
    }
}
