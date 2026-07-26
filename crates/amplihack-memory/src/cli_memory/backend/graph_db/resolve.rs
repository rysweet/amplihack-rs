use super::super::super::*;
use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};

pub(crate) fn resolve_memory_graph_db_path() -> Result<PathBuf> {
    fn validate_graph_db_override(path: PathBuf, env_var: &str) -> Result<PathBuf> {
        if !path.is_absolute() {
            bail!(
                "invalid {env_var} override: memory graph DB path must be absolute: {}",
                path.display()
            );
        }
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            bail!(
                "invalid {env_var} override: memory graph DB path must not contain parent traversal: {}",
                path.display()
            );
        }
        for blocked in [Path::new("/proc"), Path::new("/sys"), Path::new("/dev")] {
            if path.starts_with(blocked) {
                bail!(
                    "invalid {env_var} override: memory graph DB path uses blocked prefix {}: {}",
                    blocked.display(),
                    path.display()
                );
            }
        }
        Ok(path)
    }

    if let Some(path) = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH")
        && !path.is_empty()
    {
        return validate_graph_db_override(PathBuf::from(path), "AMPLIHACK_GRAPH_DB_PATH");
    }
    if let Some(path) = std::env::var_os("AMPLIHACK_KUZU_DB_PATH")
        && !path.is_empty()
    {
        return validate_graph_db_override(PathBuf::from(path), "AMPLIHACK_KUZU_DB_PATH");
    }

    let paths = memory_home_paths()?;
    if paths.legacy_graph_db.exists() && !paths.graph_db.exists() {
        return Ok(paths.legacy_graph_db);
    }
    Ok(paths.graph_db)
}
