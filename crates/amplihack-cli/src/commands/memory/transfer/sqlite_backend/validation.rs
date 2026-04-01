//! Agent name validation, path resolution, permissions, and connection helpers.

use super::schema::{MAX_AGENT_NAME_LEN, init_hierarchical_sqlite_schema};
use crate::commands::memory::ensure_parent_dir;
use anyhow::{Context, Result};
use rusqlite::Connection as SqliteConnection;
use std::path::PathBuf;

/// Validate an agent name, rejecting path traversal and invalid names.
///
/// Rules:
/// - Must not be empty.
/// - Must not exceed `MAX_AGENT_NAME_LEN` characters.
/// - Must not contain `..` (path traversal component).
/// - Must not be an absolute path (start with `/`).
pub(crate) fn validate_agent_name(agent_name: &str) -> Result<()> {
    if agent_name.is_empty() {
        anyhow::bail!("agent name must not be empty");
    }
    if agent_name.len() > MAX_AGENT_NAME_LEN {
        anyhow::bail!(
            "agent name is too long ({} characters, max {MAX_AGENT_NAME_LEN})",
            agent_name.len()
        );
    }
    // Reject absolute paths.
    if agent_name.starts_with('/') {
        anyhow::bail!("agent name must not be an absolute path: {agent_name:?}");
    }
    // Reject path traversal components.
    let path = std::path::Path::new(agent_name);
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => {
                anyhow::bail!("agent name contains path traversal component '..': {agent_name:?}");
            }
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("agent name contains absolute path component: {agent_name:?}");
            }
            _ => {}
        }
    }
    Ok(())
}

/// Resolve the SQLite database file path for a given agent.
///
/// Validation order: `validate_agent_name` is called FIRST before any
/// `PathBuf` construction.
///
/// If `storage_path` is `Some(path)`, the database lives at
/// `<storage_path>/<agent_name>.db`.
/// If `storage_path` is `None`, the database lives at
/// `~/.amplihack/hierarchical_memory/<agent_name>.db`.
pub(crate) fn resolve_hierarchical_sqlite_path(
    agent_name: &str,
    storage_path: Option<&str>,
) -> Result<PathBuf> {
    Ok(super::super::resolve_hierarchical_memory_paths(agent_name, storage_path)?.sqlite_db)
}

/// Enforce restrictive filesystem permissions on a SQLite database file.
///
/// Sets file permissions to 0o600 (owner read/write only) and the parent
/// directory to 0o700 (owner access only).
#[cfg(unix)]
pub(crate) fn enforce_hierarchical_db_permissions(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // Use symlink_metadata() (not exists()) so we detect symlinks without
    // following them.  Refuse to set permissions on a symlink — that would
    // redirect set_permissions to an attacker-controlled target file.
    if path.symlink_metadata().is_ok() {
        if path.is_symlink() {
            anyhow::bail!(
                "symlink detected at database path {}; refusing to set permissions",
                path.display()
            );
        }
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)
            .with_context(|| format!("failed to set 0o600 on {}", path.display()))?;
    }
    if let Some(parent) = path.parent()
        && parent.symlink_metadata().is_ok()
    {
        let dir_perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(parent, dir_perms)
            .with_context(|| format!("failed to set 0o700 on {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn enforce_hierarchical_db_permissions(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

/// Open a SQLite connection to the hierarchical database and initialise the
/// schema.
pub(in crate::commands::memory::transfer::sqlite_backend) fn open_hierarchical_sqlite_conn(
    db_path: &std::path::Path,
) -> Result<SqliteConnection> {
    ensure_parent_dir(db_path)?;
    let conn = SqliteConnection::open(db_path)
        .with_context(|| format!("failed to open SQLite at {}", db_path.display()))?;
    enforce_hierarchical_db_permissions(db_path)?;
    init_hierarchical_sqlite_schema(&conn)?;
    Ok(conn)
}

/// Stateless SQLite hierarchical transfer backend.
pub(crate) struct SqliteHierarchicalTransferBackend;
