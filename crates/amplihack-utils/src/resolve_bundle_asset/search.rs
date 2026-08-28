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
    // The nearest project/repo root at or above cwd.
    let cwd_root = env::current_dir().ok().and_then(|cwd| {
        cwd.ancestors()
            .find(|a| a.join("amplifier-bundle").is_dir() || a.join(".claude").is_dir())
            .map(Path::to_path_buf)
    });
    let cwd_root_is_checkout = cwd_root
        .as_deref()
        .is_some_and(crate::runtime_assets::is_source_checkout);

    ordered_bases(
        env::var("AMPLIHACK_HOME")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".amplihack")),
        cwd_root,
        cwd_root_is_checkout,
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf),
        env::current_dir().ok(),
    )
}

/// Pure ordering seam, so the precedence is testable without a real HOME, a
/// real checkout, or control over the process working directory.
///
/// Issue #1403: `~/.amplihack` used to sit ahead of the cwd-derived root
/// unconditionally. Standing inside a checkout then resolved assets out of an
/// installed tree -- one tree's code against another tree's recipes and skills,
/// silently. This is the same defect #1395 fixed in `iter_runtime_roots`, keyed
/// off cwd instead of the executable.
///
/// A demonstrable source checkout is promoted above `~/.amplihack`; anything
/// else keeps its original position, so an installed tree behaves exactly as
/// before. The checkout test itself is `runtime_assets::is_source_checkout` --
/// deliberately SHARED with #1395 rather than reimplemented, because two
/// independent definitions drifting apart is how these consumers came to
/// disagree in the first place.
fn ordered_bases(
    amplihack_home: Option<PathBuf>,
    home_amplihack: Option<PathBuf>,
    cwd_root: Option<PathBuf>,
    cwd_root_is_checkout: bool,
    workspace_root: Option<PathBuf>,
    cwd: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();

    // 1. AMPLIHACK_HOME always wins. Unchanged.
    bases.extend(amplihack_home);

    // 2. A source checkout at or above cwd outranks an installed ~/.amplihack.
    if cwd_root_is_checkout {
        bases.extend(cwd_root.clone());
    }

    // 3. ~/.amplihack
    bases.extend(home_amplihack);

    // 4. The cwd-walked root when it is NOT a checkout -- its original slot.
    if !cwd_root_is_checkout {
        bases.extend(cwd_root);
    }

    // 5. Workspace root (compile-time anchor)
    bases.extend(workspace_root);

    // 6. cwd
    bases.extend(cwd);

    // Deduplicate while preserving priority order.
    let mut seen = std::collections::HashSet::new();
    bases.retain(|p| {
        let key = p.canonicalize().unwrap_or_else(|_| p.clone());
        seen.insert(key)
    });

    bases
}

#[cfg(test)]
mod issue_1403_tests {
    use super::ordered_bases;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// The defect: standing inside a checkout, assets resolved out of an
    /// installed `~/.amplihack` -- one tree's code against another tree's
    /// recipes and skills, with no warning. Same bug as #1395, keyed off cwd.
    #[test]
    fn a_source_checkout_at_cwd_outranks_an_installed_home() {
        let bases = ordered_bases(
            None,
            Some(p("/home/u/.amplihack")),
            Some(p("/home/u/src/checkout")),
            true,
            None,
            None,
        );
        assert_eq!(
            bases.first(),
            Some(&p("/home/u/src/checkout")),
            "a checkout containing cwd must be searched before an installed \
             ~/.amplihack (issue #1403); got {bases:?}"
        );
    }

    /// The behaviour that must NOT change: an ordinary directory that merely
    /// has a `.claude/` in it is not a checkout, and an installed tree keeps
    /// winning exactly as it did before.
    #[test]
    fn a_non_checkout_root_keeps_its_original_position() {
        let bases = ordered_bases(
            None,
            Some(p("/home/u/.amplihack")),
            Some(p("/home/u/some/dir")),
            false,
            None,
            None,
        );
        assert_eq!(
            bases,
            vec![p("/home/u/.amplihack"), p("/home/u/some/dir")],
            "a non-checkout must not be promoted; installed behaviour is unchanged"
        );
    }

    #[test]
    fn amplihack_home_still_beats_everything() {
        let bases = ordered_bases(
            Some(p("/explicit/home")),
            Some(p("/home/u/.amplihack")),
            Some(p("/home/u/src/checkout")),
            true,
            None,
            None,
        );
        assert_eq!(
            bases.first(),
            Some(&p("/explicit/home")),
            "an explicit AMPLIHACK_HOME is the highest-precedence root and that \
             must survive this change"
        );
    }

    #[test]
    fn the_full_order_is_stable() {
        let bases = ordered_bases(
            Some(p("/a/home")),
            Some(p("/a/dotamplihack")),
            Some(p("/a/checkout")),
            true,
            Some(p("/a/workspace")),
            Some(p("/a/cwd")),
        );
        assert_eq!(
            bases,
            vec![
                p("/a/home"),
                p("/a/checkout"),
                p("/a/dotamplihack"),
                p("/a/workspace"),
                p("/a/cwd"),
            ]
        );
    }

    #[test]
    fn a_root_appearing_twice_is_listed_once_at_its_best_position() {
        // cwd and the checkout root are frequently the same directory.
        let bases = ordered_bases(
            None,
            Some(p("/home/u/.amplihack")),
            Some(p("/home/u/src/checkout")),
            true,
            None,
            Some(p("/home/u/src/checkout")),
        );
        assert_eq!(
            bases,
            vec![p("/home/u/src/checkout"), p("/home/u/.amplihack")],
            "dedup must keep the earliest occurrence, not demote it"
        );
    }
}
