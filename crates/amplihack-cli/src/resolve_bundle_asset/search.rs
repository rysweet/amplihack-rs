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

    // Installed runtime roots. When a recipe runs against a target repo whose
    // `amplifier-bundle/` is gitignored (empty `tools/` in git worktrees), the
    // complete bundle lives under one of these install roots — never in the
    // target checkout. `~/.copilot` is the Copilot CLI install location and
    // `~/.amplihack` is the default `amplihack install` root.
    if let Ok(home) = env::var("HOME") {
        let home = PathBuf::from(home);
        bases.push(home.join(".copilot"));
        bases.push(home.join(".amplihack"));
    }

    bases
}

/// Search bases for named assets — matches Python's `iter_runtime_roots()` order.
///
/// Priority:
/// 1. `AMPLIHACK_HOME` env var (highest priority)
/// 2. `~/.copilot` then `~/.amplihack` (installed runtime roots)
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

    // 2. Installed runtime roots (`~/.copilot` = Copilot CLI install,
    //    `~/.amplihack` = default `amplihack install`). These hold the complete
    //    bundle when the target repo's `amplifier-bundle/` is gitignored/empty.
    if let Ok(home) = env::var("HOME") {
        let home = PathBuf::from(home);
        bases.push(home.join(".copilot"));
        bases.push(home.join(".amplihack"));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for the evidence-helper discard bug: when a target repo's
    /// `amplifier-bundle/` is gitignored (empty in worktrees) and AMPLIHACK_HOME
    /// points at that repo, the complete bundle lives under an install root.
    /// `search_bases()` must include `~/.copilot` (Copilot install) ahead of
    /// `~/.amplihack` so `resolve-bundle-asset` can find the real helper.
    #[test]
    fn search_bases_includes_copilot_root_before_amplihack() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_home = crate::test_support::set_home(temp.path());

        let bases = search_bases();

        crate::test_support::restore_home(prev_home);

        let copilot_idx = bases
            .iter()
            .position(|b| *b == temp.path().join(".copilot"));
        let amplihack_idx = bases
            .iter()
            .position(|b| *b == temp.path().join(".amplihack"));
        assert!(
            copilot_idx.is_some(),
            "search_bases must include ~/.copilot: {bases:?}"
        );
        assert!(
            amplihack_idx.is_some(),
            "search_bases must include ~/.amplihack: {bases:?}"
        );
        assert!(
            copilot_idx < amplihack_idx,
            "~/.copilot must be searched before ~/.amplihack: {bases:?}"
        );
    }

    #[test]
    fn named_asset_search_bases_includes_copilot_root() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_home = crate::test_support::set_home(temp.path());

        let bases = named_asset_search_bases();

        crate::test_support::restore_home(prev_home);

        assert!(
            bases.iter().any(|b| *b == temp.path().join(".copilot")),
            "named_asset_search_bases must include ~/.copilot: {bases:?}"
        );
    }
}
