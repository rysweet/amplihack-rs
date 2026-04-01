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
pub(crate) use schema::{
    SQLITE_HIERARCHICAL_INDEXES, SQLITE_HIERARCHICAL_SCHEMA, SqlIndexStatements,
    init_hierarchical_sqlite_schema,
};
pub(crate) use validation::{
    SqliteHierarchicalTransferBackend, enforce_hierarchical_db_permissions,
    resolve_hierarchical_sqlite_path, validate_agent_name,
};
