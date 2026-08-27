//! Runtime asset resolution for amplihack recipe-runner shell commands.
//!
//! Resolves bundled assets across multiple candidate root directories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

/// Well-known asset relative paths keyed by logical asset name.
///
/// Each asset name maps to one or more candidate relative paths tried in order.
use crate::resolve_bundle_asset;

/// Well-known asset relative paths keyed by logical asset name.
///
/// Each asset name maps to one or more candidate relative paths tried in order.
pub fn asset_relative_paths() -> HashMap<&'static str, Vec<&'static str>> {
    resolve_bundle_asset::named_asset_relative_paths()
        .into_iter()
        .map(|(name, paths)| (name, paths.to_vec()))
        .collect()
}

fn relative_paths_for(asset_name: &str) -> Option<&'static [&'static str]> {
    resolve_bundle_asset::named_asset_relative_paths()
        .into_iter()
        .find(|(name, _)| *name == asset_name)
        .map(|(_, paths)| paths)
}

/// Iterate candidate runtime root directories.
///
/// Returns roots in priority order:
/// 1. `AMPLIHACK_HOME` environment variable
/// 2. The executable-adjacent root, but only when that root is demonstrably a
///    source checkout (issue #1395)
/// 3. `~/.amplihack`
/// 4. The executable-adjacent root, when it is not a source checkout
/// 5. Current working directory
///
/// Step 2 exists because a binary built from a checkout and run inside that
/// checkout must resolve its assets from the tree it was built from, not from
/// whatever happens to be installed in the user's home directory. A shipped
/// binary has no checkout markers next to it, so it keeps the historical
/// ordering: `~/.amplihack` ahead of the install root.
pub fn iter_runtime_roots() -> Vec<PathBuf> {
    runtime_roots_from(
        std::env::var("AMPLIHACK_HOME").ok(),
        home_dir(),
        std::env::current_exe().ok(),
        std::env::current_dir().ok(),
    )
}

/// Environment-free core of [`iter_runtime_roots`], so the ordering is testable
/// without mutating process-global state.
fn runtime_roots_from(
    amplihack_home: Option<String>,
    home_dir: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    current_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    // 1. AMPLIHACK_HOME env var — always the highest-precedence root.
    if let Some(home) = amplihack_home {
        let p = PathBuf::from(&home);
        if p.is_dir() {
            debug!(path = %p.display(), "runtime root from AMPLIHACK_HOME");
            roots.push(p);
        }
    }

    // The bundle root above the executable, and whether it is a source checkout.
    let package_root = current_exe.as_deref().and_then(package_root_above);
    let checkout_root = package_root
        .as_deref()
        .filter(|root| is_source_checkout(root))
        .map(Path::to_path_buf);

    // 2. Source checkout containing the executable.
    if let Some(root) = &checkout_root {
        debug!(
            path = %root.display(),
            "runtime root from the source checkout containing the executable"
        );
        roots.push(root.clone());
    }

    // 3. ~/.amplihack
    if let Some(home_dir) = home_dir {
        let dot_amplihack = home_dir.join(".amplihack");
        if dot_amplihack.is_dir() {
            debug!(path = %dot_amplihack.display(), "runtime root from ~/.amplihack");
            roots.push(dot_amplihack);
        }
    }

    // 4. Package root above the executable, when it is not a source checkout.
    if checkout_root.is_none()
        && let Some(root) = package_root
    {
        debug!(path = %root.display(), "runtime root from package hierarchy");
        roots.push(root);
    }

    // 5. Current working directory
    if let Some(cwd) = current_dir {
        debug!(path = %cwd.display(), "runtime root from cwd");
        roots.push(cwd);
    }

    dedupe_preserving_order(roots)
}

/// Walk up from an executable path looking for a directory that holds
/// `amplifier-bundle/`.
fn package_root_above(exe: &Path) -> Option<PathBuf> {
    let mut dir = exe.parent();
    while let Some(d) = dir {
        if d.join("amplifier-bundle").is_dir() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Is `root` demonstrably a source checkout rather than an installed tree?
///
/// Requires the bundle directory plus a checkout marker: a `.git` entry (a
/// directory in a normal clone, a file in a worktree or submodule) or a
/// `Cargo.toml` that declares a `[workspace]`. An installed tree has neither,
/// so installed binaries keep their historical resolution order.
fn is_source_checkout(root: &Path) -> bool {
    if !root.join("amplifier-bundle").is_dir() {
        return false;
    }
    root.join(".git").exists() || cargo_toml_declares_workspace(root)
}

/// Does `root/Cargo.toml` contain a `[workspace]` table header?
fn cargo_toml_declares_workspace(root: &Path) -> bool {
    let manifest = root.join("Cargo.toml");
    // Guard against reading something enormous that merely shares the name.
    match std::fs::metadata(&manifest) {
        Ok(meta) if meta.is_file() && meta.len() <= MAX_MANIFEST_BYTES => {}
        _ => return false,
    }
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return false;
    };
    text.lines()
        .any(|line| matches!(line.trim(), "[workspace]" | "[ workspace ]"))
}

/// Largest `Cargo.toml` inspected for a `[workspace]` header.
const MAX_MANIFEST_BYTES: u64 = 1 << 20;

/// Largest asset compared byte-for-byte when checking whether two roots agree.
const MAX_ASSET_COMPARE_BYTES: u64 = 1 << 20;

fn dedupe_preserving_order(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    roots
        .into_iter()
        .filter(|p| seen.insert(p.canonicalize().unwrap_or_else(|_| p.clone())))
        .collect()
}

/// Resolve the first existing path for a named asset across search roots.
///
/// Tries each relative path variant under each root in order. When a
/// lower-priority root supplies a *different* copy of the same asset, that
/// disagreement is logged (issue #1395) — it is never fatal.
pub fn resolve_asset_path(asset_name: &str, search_roots: &[PathBuf]) -> Result<PathBuf> {
    let rel_paths = relative_paths_for(asset_name)
        .with_context(|| format!("unknown asset name: {asset_name}"))?;

    for (idx, root) in search_roots.iter().enumerate() {
        for rel in rel_paths {
            let candidate = root.join(rel);
            if candidate.exists() {
                info!(
                    asset = asset_name,
                    path = %candidate.display(),
                    "resolved asset"
                );
                warn_on_shadowed_asset(asset_name, &candidate, &search_roots[idx + 1..], rel_paths);
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

/// Warn once when a lower-priority root supplies a different copy of the asset
/// that was just resolved.
fn warn_on_shadowed_asset(
    asset_name: &str,
    chosen: &Path,
    lower_roots: &[PathBuf],
    rel_paths: &[&str],
) {
    for root in lower_roots {
        for rel in rel_paths {
            let other = root.join(rel);
            if other.exists() && assets_differ(chosen, &other) {
                warn!(
                    asset = asset_name,
                    using = %chosen.display(),
                    shadowed = %other.display(),
                    "runtime roots disagree about this asset; using the higher-priority root"
                );
                return;
            }
        }
    }
}

/// Best-effort comparison of two copies of the same asset. Returns `false`
/// (treat as identical) whenever the answer cannot be established cheaply, so
/// the warning stays quiet rather than crying wolf.
fn assets_differ(a: &Path, b: &Path) -> bool {
    if let (Ok(ca), Ok(cb)) = (a.canonicalize(), b.canonicalize())
        && ca == cb
    {
        return false;
    }
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if ma.is_dir() && mb.is_dir() {
        return dir_entry_names(a) != dir_entry_names(b);
    }
    if ma.is_file() && mb.is_file() {
        if ma.len() != mb.len() {
            return true;
        }
        if ma.len() > MAX_ASSET_COMPARE_BYTES {
            return false;
        }
        return match (std::fs::read(a), std::fs::read(b)) {
            (Ok(da), Ok(db)) => da != db,
            _ => false,
        };
    }
    ma.is_dir() != mb.is_dir()
}

/// Sorted names of the direct children of `dir`, or `None` if unreadable.
fn dir_entry_names(dir: &Path) -> Option<Vec<std::ffi::OsString>> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name())
        .collect();
    names.sort();
    Some(names)
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
            eprintln!("error: {e}");
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
        assert!(paths.contains_key("multitask-orchestrator"));
        assert!(paths.contains_key("helper-path"));
        assert!(paths.contains_key("session-tree-path"));
        assert!(paths.contains_key("hooks-dir"));
    }

    #[test]
    fn multitask_orchestrator_uses_native_wrapper() {
        let paths = asset_relative_paths();
        let orch = &paths["multitask-orchestrator"];
        assert_eq!(orch.len(), 1);
        assert!(orch[0].contains("amplifier-bundle/bin"));
    }

    #[test]
    fn helper_path_uses_native_wrapper() {
        let paths = asset_relative_paths();
        let helper = &paths["helper-path"];
        assert_eq!(
            helper,
            &vec!["amplifier-bundle/bin/multitask-orchestrator.sh"]
        );
    }

    #[test]
    fn hooks_dir_is_registered_for_legacy_asset_resolution() {
        let paths = asset_relative_paths();
        assert!(
            paths.contains_key("hooks-dir"),
            "hooks-dir asset must remain registered for issue #634 parity"
        );
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
        let result = resolve_asset_path("multitask-orchestrator", &roots);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));
    }

    // --- issue #1395: root ordering when running from a source checkout ---

    /// Build a fake bundle root: `<root>/amplifier-bundle/bin/<script>` with
    /// the given contents, plus a fake executable under `<root>/target/debug/`.
    fn make_root(root: &Path, script_body: &str) -> PathBuf {
        let bin = root.join("amplifier-bundle/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("multitask-orchestrator.sh"), script_body).unwrap();
        let exe = root.join("target/debug/amplihack");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        exe
    }

    #[test]
    fn source_checkout_containing_the_executable_outranks_installed_home() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        let exe = make_root(&checkout, "from the checkout\n");
        std::fs::write(checkout.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();

        let home = tmp.path().join("home");
        make_root(&home.join(".amplihack"), "from the install\n");

        let roots = runtime_roots_from(None, Some(home.clone()), Some(exe), None);

        assert_eq!(
            roots.first().map(PathBuf::as_path),
            Some(checkout.as_path()),
            "a binary built from a checkout must resolve assets from that checkout, \
             not from ~/.amplihack (issue #1395); got {roots:?}"
        );
        assert!(
            roots.contains(&home.join(".amplihack")),
            "~/.amplihack must remain a root, just a lower-priority one"
        );

        let resolved = resolve_asset_path("multitask-orchestrator", &roots).unwrap();
        assert!(
            resolved.starts_with(&checkout),
            "resolved {} should live under the checkout",
            resolved.display()
        );
    }

    #[test]
    fn a_git_worktree_marker_file_counts_as_a_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        let exe = make_root(&checkout, "checkout\n");
        // Worktrees and submodules have a `.git` FILE, not a directory.
        std::fs::write(checkout.join(".git"), "gitdir: /elsewhere\n").unwrap();

        let home = tmp.path().join("home");
        make_root(&home.join(".amplihack"), "install\n");

        let roots = runtime_roots_from(None, Some(home), Some(exe), None);
        assert_eq!(
            roots.first().map(PathBuf::as_path),
            Some(checkout.as_path())
        );
    }

    #[test]
    fn installed_binary_keeps_dot_amplihack_ahead_of_its_own_root() {
        let tmp = tempfile::tempdir().unwrap();
        // An installed tree: bundle present, no checkout markers.
        let install = tmp.path().join("opt/amplihack");
        let exe = make_root(&install, "install prefix\n");

        let home = tmp.path().join("home");
        let dot = home.join(".amplihack");
        make_root(&dot, "user install\n");

        let roots = runtime_roots_from(None, Some(home), Some(exe), None);
        assert_eq!(
            roots,
            vec![dot, install],
            "shipped binaries must keep the historical order"
        );
    }

    #[test]
    fn a_bare_cargo_toml_without_a_workspace_table_is_not_a_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("opt/amplihack");
        let exe = make_root(&install, "install\n");
        std::fs::write(install.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        let home = tmp.path().join("home");
        let dot = home.join(".amplihack");
        make_root(&dot, "user install\n");

        let roots = runtime_roots_from(None, Some(home), Some(exe), None);
        assert_eq!(roots, vec![dot, install]);
    }

    #[test]
    fn amplihack_home_still_outranks_a_source_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let explicit = tmp.path().join("explicit");
        make_root(&explicit, "explicit\n");

        let checkout = tmp.path().join("checkout");
        let exe = make_root(&checkout, "checkout\n");
        std::fs::write(checkout.join("Cargo.toml"), "[workspace]\n").unwrap();

        let home = tmp.path().join("home");
        make_root(&home.join(".amplihack"), "install\n");

        let roots = runtime_roots_from(
            Some(explicit.to_string_lossy().into_owned()),
            Some(home),
            Some(exe),
            None,
        );
        assert_eq!(
            roots.first().map(PathBuf::as_path),
            Some(explicit.as_path())
        );
    }

    #[test]
    fn roots_are_deduplicated_in_priority_order() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        let exe = make_root(&checkout, "checkout\n");
        std::fs::write(checkout.join(".git"), "gitdir: x\n").unwrap();

        // cwd is the checkout too — it must not appear twice.
        let roots = runtime_roots_from(None, None, Some(exe), Some(checkout.clone()));
        assert_eq!(roots, vec![checkout]);
    }

    #[test]
    fn differing_copies_of_an_asset_are_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.sh");
        let b = tmp.path().join("b.sh");
        let same = tmp.path().join("same.sh");
        std::fs::write(&a, "one\n").unwrap();
        std::fs::write(&b, "two\n").unwrap();
        std::fs::write(&same, "one\n").unwrap();

        assert!(assets_differ(&a, &b));
        assert!(!assets_differ(&a, &same));
        assert!(!assets_differ(&a, &a));
        // A missing side is not a disagreement.
        assert!(!assets_differ(&a, &tmp.path().join("nope.sh")));
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
