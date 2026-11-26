//! Compatibility notices.
//!
//! Issue #2279 (C13): session start no longer removes amplihack's global
//! hooks from `~/.claude/settings.json` when the project has its own copy.
//! That removal wrote the hook-less home file and fought the launch repair,
//! the rotator heal and the doctor check. Both files now keep the hooks.

use amplihack_memory::cli_memory::default_code_graph_db_path_for_project;
use amplihack_types::ProjectDirs;
use std::path::PathBuf;

pub(super) fn code_graph_compatibility_notice(
    dirs: &ProjectDirs,
) -> anyhow::Result<Option<String>> {
    let graph_override = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    if graph_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(None);
    }

    let legacy_override = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    if legacy_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(Some(super::format_code_graph_status(
            "Using legacy `AMPLIHACK_KUZU_DB_PATH` compatibility alias for the code graph. Prefer `AMPLIHACK_GRAPH_DB_PATH`.".to_string(),
        )));
    }

    let neutral = default_code_graph_db_path_for_project(&dirs.root)?;
    let legacy = dirs.root.join(".amplihack").join("kuzu_db");
    if legacy.exists() && !neutral.exists() {
        return Ok(Some(super::format_code_graph_status(format!(
            "Using legacy code-graph store `{}` because `{}` is absent. Migrate to the neutral `graph_db` path to leave compatibility mode.",
            legacy.display(),
            neutral.display()
        ))));
    }

    Ok(None)
}

pub(super) fn memory_graph_compatibility_notice() -> Option<String> {
    if std::env::var("AMPLIHACK_MEMORY_BACKEND").ok().as_deref() == Some("sqlite") {
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
        return Some(super::format_memory_status(
            "Using legacy `AMPLIHACK_KUZU_DB_PATH` compatibility alias for the memory graph. Prefer `AMPLIHACK_GRAPH_DB_PATH`.".to_string(),
        ));
    }

    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let neutral = home.join(".amplihack").join("memory_graph.db");
    let legacy = home.join(".amplihack").join("memory_kuzu.db");
    if legacy.exists() && !neutral.exists() {
        return Some(super::format_memory_status(format!(
            "Using legacy memory graph store `{}` because `{}` is absent. Migrate to `memory_graph.db` to leave compatibility mode.",
            legacy.display(),
            neutral.display()
        )));
    }

    None
}
