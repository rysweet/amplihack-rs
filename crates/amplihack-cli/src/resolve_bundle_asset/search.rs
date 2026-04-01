//! Search-base resolution for bundle asset lookups.

use std::env;
use std::path::{Path, PathBuf};

pub(super) fn search_bases() -> Vec<PathBuf> {
    let mut bases = Vec::new();

    if let Ok(amplihack_home) = env::var("AMPLIHACK_HOME") {
        let path = PathBuf::from(amplihack_home);
        if path.is_dir() {
            bases.push(path);
        }
    }

    if let Ok(cwd) = env::current_dir() {
        for ancestor in cwd.ancestors() {
            if ancestor.join("amplifier-bundle").is_dir() {
                bases.push(ancestor.to_path_buf());
                break;
            }
        }
    }

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    if let Some(root) = workspace_root {
        bases.push(root);
    }

    if let Ok(home) = env::var("HOME") {
        bases.push(PathBuf::from(home).join(".amplihack"));
    }

    bases
}

/// Search bases for named assets — matches Python's `iter_runtime_roots()` order.
///
/// Priority:
/// 1. `AMPLIHACK_HOME` env var (highest priority)
/// 2. `~/.amplihack`
/// 3. Walk up from cwd until a project root marker is found
/// 4. Workspace root (compile-time anchor, analogous to Python's package/repo root)
/// 5. cwd
pub(super) fn named_asset_search_bases() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();

    // 1. AMPLIHACK_HOME env var
    if let Ok(amplihack_home) = env::var("AMPLIHACK_HOME")
        && !amplihack_home.is_empty()
    {
        bases.push(PathBuf::from(amplihack_home));
    }

    // 2. ~/.amplihack
    if let Ok(home) = env::var("HOME") {
        bases.push(PathBuf::from(home).join(".amplihack"));
    }

    // 3. Walk up from cwd looking for a project/repo root marker
    if let Ok(cwd) = env::current_dir() {
        for ancestor in cwd.ancestors() {
            if ancestor.join("amplifier-bundle").is_dir() || ancestor.join(".claude").is_dir() {
                bases.push(ancestor.to_path_buf());
                break;
            }
        }
    }

    // 4. Workspace root (compile-time anchor)
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    if let Some(root) = workspace_root {
        bases.push(root);
    }

    // 5. cwd
    if let Ok(cwd) = env::current_dir() {
        bases.push(cwd);
    }

    // Deduplicate while preserving priority order
    let mut seen = std::collections::HashSet::new();
    bases.retain(|p| {
        let key = p.canonicalize().unwrap_or_else(|_| p.clone());
        seen.insert(key)
    });

    bases
}
