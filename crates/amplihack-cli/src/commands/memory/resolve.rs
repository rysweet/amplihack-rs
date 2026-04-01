use anyhow::{Context, Result};

use super::helpers::parse_backend_choice_env_value;
use super::types::{
    backend_cli_compatibility_notice, memory_home_paths, BackendChoice, ResolvedMemoryCliBackend,
};

/// Resolve the memory backend from `AMPLIHACK_MEMORY_BACKEND`.
///
/// Returns:
/// - `Ok(Some(choice))` — recognised value.
/// - `Ok(None)` — env var not set; caller picks a default.
/// - `Err(...)` — env var is set but unrecognised. This is a hard error:
///   silently activating the wrong backend on a typo (e.g. `sqllite`)
///   risks routing data to an unexpected location.
pub(super) fn resolve_memory_backend_preference() -> Result<Option<BackendChoice>> {
    match std::env::var("AMPLIHACK_MEMORY_BACKEND").ok().as_deref() {
        Some(value) => Ok(Some(parse_backend_choice_env_value(value)?)),
        None => Ok(None),
    }
}

pub(crate) fn resolve_memory_cli_backend(backend: &str) -> Result<ResolvedMemoryCliBackend> {
    let choice = if backend == "auto" {
        resolve_backend_with_autodetect()?
    } else {
        BackendChoice::parse(backend)?
    };
    Ok(ResolvedMemoryCliBackend {
        choice,
        cli_notice: backend_cli_compatibility_notice(backend),
        graph_notice: memory_graph_compatibility_notice(choice),
    })
}

pub(crate) fn memory_graph_compatibility_notice(choice: BackendChoice) -> Option<String> {
    if !matches!(choice, BackendChoice::GraphDb) {
        return None;
    }

    let graph_override = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    if graph_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return None;
    }

    let legacy_override = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    if legacy_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Some(
            "using legacy `AMPLIHACK_KUZU_DB_PATH`; prefer `AMPLIHACK_GRAPH_DB_PATH`.".to_string(),
        );
    }

    let paths = memory_home_paths().ok()?;
    if paths.legacy_graph_db.exists() && !paths.graph_db.exists() {
        return Some(format!(
            "using legacy store `{}` because `{}` is absent; migrate to `memory_graph.db`.",
            paths.legacy_graph_db.display(),
            paths.graph_db.display()
        ));
    }

    None
}

/// Resolve the memory backend with autodetection.
///
/// Resolution order:
/// 1. `AMPLIHACK_MEMORY_BACKEND` env var (if set and recognized).
/// 2. Probe `~/.amplihack/hierarchical_memory/` for existing legacy graph-db
///    directories using `symlink_metadata()` (not `exists()`).
///    - If a symlink is found inside the probe directory → return `Err`.
///    - If a `graph_db` subdirectory is found → `BackendChoice::GraphDb`.
/// 3. Default to `BackendChoice::Sqlite` for new installs.
///
/// Returns `Err` if `HOME` is unavailable (only checked when the env var
/// shortcut is not used).
pub(crate) fn resolve_backend_with_autodetect() -> Result<BackendChoice> {
    // Step 1: env var takes priority (returns Err on unrecognised value).
    if let Some(choice) = resolve_memory_backend_preference()? {
        return Ok(choice);
    }

    // Step 2: probe the filesystem.
    let hmem_dir = memory_home_paths()?.hierarchical_memory_dir;

    // If the directory doesn't exist at all, this is a fresh install.
    if hmem_dir.symlink_metadata().is_err() {
        return Ok(BackendChoice::Sqlite);
    }

    // Scan the hierarchical_memory directory for agent subdirectories.
    // Use symlink_metadata() on each entry to detect symlinks.
    for entry_result in std::fs::read_dir(&hmem_dir)
        .with_context(|| format!("failed to read directory {}", hmem_dir.display()))?
    {
        let entry = entry_result
            .with_context(|| format!("failed to read entry in {}", hmem_dir.display()))?;
        let entry_path = entry.path();

        // Use symlink_metadata() to detect symlinks without following them.
        let meta = entry_path
            .symlink_metadata()
            .with_context(|| format!("failed to stat {}", entry_path.display()))?;

        if meta.file_type().is_symlink() {
            anyhow::bail!(
                "symlink detected in backend probe path {}; refusing to follow for security",
                entry_path.display()
            );
        }

        if meta.is_dir() {
            // Check if this agent directory contains a graph_db subdirectory.
            let graph_db = entry_path.join("graph_db");
            if graph_db.symlink_metadata().is_ok() {
                return Ok(BackendChoice::GraphDb);
            }
        }
    }

    // Step 3: No legacy graph-db markers found -> default to SQLite.
    Ok(BackendChoice::Sqlite)
}
