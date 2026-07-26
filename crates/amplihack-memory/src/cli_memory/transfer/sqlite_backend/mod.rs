//! SQLite hierarchical transfer backend.
//!
//! Implements `HierarchicalTransferBackend` for SQLite, mirroring the graph-db
//! implementation semantics so both backends are interchangeable for
//! export/import workflows.

mod import_helpers;
mod loaders;
mod operations;
mod schema;
mod validation;

// Re-export public items so external paths (`sqlite_backend::Foo`) remain stable.
#[cfg(test)]
pub(crate) use schema::{
    SQLITE_HIERARCHICAL_INDEXES, SQLITE_HIERARCHICAL_SCHEMA, init_hierarchical_sqlite_schema,
};
pub(crate) use validation::{SqliteHierarchicalTransferBackend, validate_agent_name};
#[cfg(test)]
pub(crate) use validation::{
    enforce_hierarchical_db_permissions, resolve_hierarchical_sqlite_path,
};
