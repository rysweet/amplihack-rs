//! Runtime asset resolution for amplihack recipe-runner shell commands.
//!
//! Ports Python `amplihack/runtime_assets.py`: resolves bundled assets
//! (helper scripts, hooks directories) across multiple candidate root
//! directories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

/// Well-known asset relative paths keyed by logical asset name.
///
/// Each asset name maps to one or more candidate relative paths tried in order.
pub fn asset_relative_paths() -> HashMap<&'static str, Vec<&'static str>> {
    let mut m = HashMap::new();
    m.insert("helper-path", vec!["amplifier-bundle/tools/orch_helper.py"]);
    m.insert(
        "session-tree-path",
        vec!["amplifier-bundle/tools/session_tree.py"],
    );
    m.insert(
        "hooks-dir",
        vec![
            ".claude/tools/amplihack/hooks",
            "amplifier-bundle/tools/amplihack/hooks",
        ],
    );
    // FIX (rysweet/amplihack-rs#283/#248): expose the multitask-orchestrator
    // script so smart-orchestrator.yaml can drop its remaining
    // `python3 -m amplihack.runtime_assets multitask-orchestrator` shim.
    m.insert(
        "multitask-orchestrator",
        vec![
            ".claude/skills/multitask/orchestrator.py",
            "amplifier-bundle/skills/multitask/orchestrator.py",
        ],
    );
    m
}

/// Iterate candidate runtime root directories.
///
/// Returns roots in priority order:
/// 1. `AMPLIHACK_HOME` environment variable
/// 2. `~/.amplihack`
/// 3. Package root (walk up from executable to find `amplifier-bundle/`)
/// 4. Current working directory
pub fn iter_runtime_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    // 1. AMPLIHACK_HOME env var
    if let Ok(home) = std::env::var("AMPLIHACK_HOME") {
        let p = PathBuf::from(&home);
        if p.is_dir() {
            debug!(path = %p.display(), "runtime root from AMPLIHACK_HOME");
            roots.push(p);
        }
    }

    // 2. ~/.amplihack
    if let Some(home_dir) = home_dir() {
        let dot_amplihack = home_dir.join(".amplihack");
        if dot_amplihack.is_dir() {
            debug!(path = %dot_amplihack.display(), "runtime root from ~/.amplihack");
            roots.push(dot_amplihack);
        }
    }

    // 3. Walk up from executable looking for amplifier-bundle/
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        while let Some(d) = dir {
            if d.join("amplifier-bundle").is_dir() {
                debug!(path = %d.display(), "runtime root from package hierarchy");
                roots.push(d.clone());
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }

    // 4. Current working directory
    if let Ok(cwd) = std::env::current_dir() {
        debug!(path = %cwd.display(), "runtime root from cwd");
        roots.push(cwd);
    }

    roots
}

/// Resolve the first existing path for a named asset across search roots.
///
/// Tries each relative path variant under each root in order.
pub fn resolve_asset_path(asset_name: &str, search_roots: &[PathBuf]) -> Result<PathBuf> {
    let asset_map = asset_relative_paths();
    let rel_paths = asset_map
        .get(asset_name)
        .with_context(|| format!("unknown asset name: {}", asset_name))?;

    for root in search_roots {
        for rel in rel_paths {
            let candidate = root.join(rel);
            if candidate.exists() {
                info!(
                    asset = asset_name,
                    path = %candidate.display(),
                    "resolved asset"
                );
                return Ok(candidate);
            }
        }
    }

    anyhow::bail!(
        "asset '{}' not found in {} root(s); tried paths: {:?}",
        asset_name,
        search_roots.len(),
        rel_paths
    )
}

/// CLI entry point for recipe shell commands that resolve and print asset paths.
///
/// Usage: `runtime_assets <asset-name> [--roots <dir>,...]`
///
/// Returns 0 on success, 1 on failure.
pub fn main(argv: &[String]) -> i32 {
    if argv.is_empty() {
        eprintln!("usage: runtime_assets <asset-name> [--roots <dir>,...]");
        return 1;
    }

    let asset_name = &argv[0];
    let roots = if argv.len() >= 3 && argv[1] == "--roots" {
        argv[2].split(',').map(PathBuf::from).collect::<Vec<_>>()
    } else {
        iter_runtime_roots()
    };

    match resolve_asset_path(asset_name, &roots) {
        Ok(path) => {
            println!("{}", path.display());
            0
        }
        Err(e) => {
            warn!(error = %e, "asset resolution failed");
            eprintln!("error: {}", e);
            1
        }
    }
}

/// Cross-platform home directory helper.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_relative_paths_has_known_keys() {
        let paths = asset_relative_paths();
        assert!(paths.contains_key("helper-path"));
        assert!(paths.contains_key("session-tree-path"));
        assert!(paths.contains_key("hooks-dir"));
        assert!(paths.contains_key("multitask-orchestrator"));
    }

    #[test]
    fn multitask_orchestrator_has_two_candidates() {
        let paths = asset_relative_paths();
        let orch = &paths["multitask-orchestrator"];
        assert_eq!(orch.len(), 2);
        assert!(orch[0].contains(".claude/skills/multitask"));
        assert!(orch[1].contains("amplifier-bundle/skills/multitask"));
    }

    #[test]
    fn hooks_dir_has_two_candidates() {
        let paths = asset_relative_paths();
        let hooks = &paths["hooks-dir"];
        assert_eq!(hooks.len(), 2);
        assert!(hooks[0].contains("claude/tools"));
        assert!(hooks[1].contains("amplifier-bundle"));
    }

    #[test]
    fn iter_runtime_roots_returns_at_least_cwd() {
        let roots = iter_runtime_roots();
        // At minimum, cwd should be present
        assert!(!roots.is_empty(), "should find at least cwd");
    }

    #[test]
    fn resolve_asset_unknown_name_fails() {
        let roots = vec![PathBuf::from(".")];
        let result = resolve_asset_path("nonexistent-asset", &roots);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unknown asset name"));
    }

    #[test]
    fn resolve_asset_missing_file_fails() {
        let roots = vec![PathBuf::from("/unlikely/to/exist/path")];
        let result = resolve_asset_path("helper-path", &roots);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));
    }

    #[test]
    fn main_no_args_returns_1() {
        assert_eq!(main(&[]), 1);
    }

    #[test]
    fn main_unknown_asset_returns_1() {
        let args = vec!["bogus-asset-name".to_string()];
        assert_eq!(main(&args), 1);
    }
}
