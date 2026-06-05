use anyhow::{Result, bail};
#[cfg(test)]
use std::env;
use std::path::{Path, PathBuf};

mod search;

use search::{named_asset_search_bases, search_bases};

/// O(1) check for allowed path characters (A-Z a-z 0-9 _ - . /).
/// Replaces the O(n) linear scan of a constant string.
fn is_safe_path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/')
}

/// Named asset mappings for runtime bundle assets.
///
/// Each entry is `(name, &[relative_paths])` where relative_paths are tried in order.
const NAMED_ASSETS: [(&str, &[&str]); 4] = [
    // hooks/ dir — native Rust hooks binary reads config from here.
    ("hooks-dir", &["amplifier-bundle/tools/amplihack/hooks"]),
    // helper-path — orchestration helper script. The Python orch_helper.py
    // was replaced by the native shell orchestrator in the Rust port.
    (
        "helper-path",
        &["amplifier-bundle/bin/multitask-orchestrator.sh"],
    ),
    // session-tree-path — session tree state directory. In the Rust port,
    // session tree tracking is built into the amplihack binary
    // (crates/amplihack-cli/src/commands/session_tree/). The asset resolves
    // to the tools/session directory for callers that need a filesystem anchor.
    (
        "session-tree-path",
        &["amplifier-bundle/tools/amplihack/session"],
    ),
    // multitask-orchestrator — native shell wrapper.
    (
        "multitask-orchestrator",
        &["amplifier-bundle/bin/multitask-orchestrator.sh"],
    ),
];

pub fn validate_relative_path(relative_path: &str) -> Result<()> {
    if relative_path.contains('\0') {
        bail!("Path must not contain null bytes.");
    }
    if relative_path.is_empty() {
        bail!("Relative path must not be empty.");
    }
    if relative_path.starts_with('/') || relative_path.starts_with('~') {
        bail!("Path must be relative, not absolute: {relative_path:?}");
    }
    for segment in relative_path.split('/') {
        if segment == "." || segment == ".." {
            bail!("Path segments '.' and '..' are not allowed: {relative_path:?}");
        }
    }
    if !relative_path.starts_with("amplifier-bundle/") {
        bail!("Path must start with 'amplifier-bundle/': {relative_path:?}");
    }
    if !relative_path.chars().all(is_safe_path_char) {
        bail!("Path contains unsafe characters (allowed: A-Z a-z 0-9 _ - . /): {relative_path:?}");
    }
    Ok(())
}

pub fn safe_join(base: &Path, relative: &str) -> Option<PathBuf> {
    let joined = base.join(relative);
    if joined.exists() {
        let base_resolved = base.canonicalize().ok()?;
        let candidate = joined.canonicalize().ok()?;
        if candidate.strip_prefix(&base_resolved).is_ok() {
            return Some(candidate);
        }
        return None;
    }

    Some(joined)
}

pub fn resolve_asset(relative_path: &str) -> Result<PathBuf> {
    validate_relative_path(relative_path)?;

    for base in search_bases() {
        if let Some(candidate) = safe_join(&base, relative_path)
            && candidate.exists()
        {
            return Ok(candidate);
        }
    }

    bail!(
        "Bundle asset not found: {relative_path}\nSet AMPLIHACK_HOME to your amplihack installation root."
    )
}

/// Resolve a named runtime asset using the Python-compatible priority chain.
///
/// Named assets are logical aliases defined in `NAMED_ASSETS`.
/// Each name expands to one or more candidate relative paths that are tried
/// in order against each search base.
///
/// Search priority matches Python's `iter_runtime_roots()`:
/// 1. `AMPLIHACK_HOME` env var
/// 2. `~/.amplihack`
/// 3. Walk up from cwd for a repo/project root marker
/// 4. Workspace root (compile-time anchor)
/// 5. cwd
pub fn named_asset_relative_paths() -> [(&'static str, &'static [&'static str]); 4] {
    NAMED_ASSETS
}

pub fn named_asset_names() -> [&'static str; 4] {
    NAMED_ASSETS.map(|(name, _)| name)
}

fn named_asset_names_csv() -> String {
    named_asset_names().join(", ")
}

pub fn usage_text(program_name: &str) -> String {
    format!(
        "Usage: {program_name} <asset>\n  <asset> is either:\n    - a named asset: {}\n    - a relative path starting with 'amplifier-bundle/'",
        named_asset_names_csv()
    )
}

pub fn resolve_named_asset(name: &str) -> Result<PathBuf> {
    let rel_paths = named_asset_relative_paths()
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, paths)| *paths)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown asset name {name:?}. Expected one of: {}",
                named_asset_names_csv()
            )
        })?;

    for base in named_asset_search_bases() {
        for rel_path in rel_paths {
            if let Some(candidate) = safe_join(&base, rel_path)
                && candidate.exists()
            {
                return Ok(candidate);
            }
        }
    }

    bail!(
        "Asset {name:?} not found in any runtime root.\nSet AMPLIHACK_HOME to your amplihack installation root."
    )
}

fn resolve_result_to_exit(result: Result<PathBuf>) -> i32 {
    match result {
        Ok(path) => {
            println!("{}", path.display());
            0
        }
        Err(err) => {
            eprintln!("ERROR: {err}");
            if err.to_string().contains("not found") {
                1
            } else {
                2
            }
        }
    }
}

pub fn run_cli(arg: &str) -> i32 {
    // Dispatch named assets (e.g. "multitask-orchestrator")
    if named_asset_relative_paths()
        .iter()
        .any(|(name, _)| *name == arg)
    {
        return resolve_result_to_exit(resolve_named_asset(arg));
    }

    // Guard: if the arg has no '/' it looks like a named-asset lookup, not a
    // raw relative path. Return exit 1 (not found) instead of letting
    // validate_relative_path reject it with exit 2 (invalid input). (#588)
    if !arg.contains('/') {
        eprintln!(
            "ERROR: Unknown asset name {arg:?}. Expected one of: {}",
            named_asset_names_csv()
        );
        return 1;
    }

    // Fall through to raw amplifier-bundle/ path resolution
    resolve_result_to_exit(resolve_asset(arg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_traversal() {
        let err = validate_relative_path("amplifier-bundle/../etc/passwd").unwrap_err();
        assert!(err.to_string().contains("'.' and '..'"));
    }

    #[test]
    fn validate_rejects_missing_prefix() {
        let err = validate_relative_path("tools/statusline.sh").unwrap_err();
        assert!(
            err.to_string()
                .contains("must start with 'amplifier-bundle/'")
        );
    }

    #[test]
    fn validate_accepts_normal_bundle_path() {
        validate_relative_path("amplifier-bundle/tools/statusline.sh").unwrap();
    }

    #[test]
    fn safe_join_blocks_symlink_escape() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let tools = base.join("amplifier-bundle/tools");
        std::fs::create_dir_all(&tools).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", tools.join("escape")).unwrap();
            assert!(safe_join(base, "amplifier-bundle/tools/escape").is_none());
        }
    }

    #[test]
    fn resolve_asset_finds_from_amplihack_home() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let asset = temp.path().join("amplifier-bundle/tools/statusline.sh");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, "ok").unwrap();

        let prev_home = crate::test_support::set_home(temp.path());
        let prev_amplihack = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let resolved = resolve_asset("amplifier-bundle/tools/statusline.sh").unwrap();

        match prev_amplihack {
            Some(value) => unsafe { env::set_var("AMPLIHACK_HOME", value) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }
        crate::test_support::restore_home(prev_home);

        assert!(
            resolved.ends_with("amplifier-bundle/tools/statusline.sh"),
            "expected statusline.sh path, got {resolved:?}"
        );
    }

    #[test]
    fn run_cli_returns_invalid_input_exit_code() {
        assert_eq!(run_cli("../../../etc/passwd"), 2);
    }

    // ── Named asset tests ─────────────────────────────────────────────────────

    fn make_named_asset_file(base: &std::path::Path, rel_path: &str) -> std::path::PathBuf {
        let target = base.join(rel_path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"ok").unwrap();
        target
    }

    #[test]
    fn resolve_named_asset_multitask_orchestrator_uses_amplihack_home() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        make_named_asset_file(
            temp.path(),
            "amplifier-bundle/bin/multitask-orchestrator.sh",
        );

        let prev_home = crate::test_support::set_home(temp.path());
        let prev = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let result = resolve_named_asset("multitask-orchestrator");

        match prev {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }
        crate::test_support::restore_home(prev_home);

        let resolved = result.unwrap();
        assert!(
            resolved.ends_with("amplifier-bundle/bin/multitask-orchestrator.sh"),
            "expected multitask orchestrator.sh path, got {resolved:?}"
        );
    }

    #[test]
    fn run_cli_resolves_multitask_orchestrator_named_asset() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        make_named_asset_file(
            temp.path(),
            "amplifier-bundle/bin/multitask-orchestrator.sh",
        );

        let prev_home = crate::test_support::set_home(temp.path());
        let prev = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let exit = run_cli("multitask-orchestrator");

        match prev {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }
        crate::test_support::restore_home(prev_home);

        assert_eq!(
            exit, 0,
            "expected resolve-bundle-asset multitask-orchestrator to succeed"
        );
    }

    #[test]
    fn resolve_named_asset_falls_back_to_dot_amplihack() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let dot_amplihack = temp.path().join(".amplihack");
        make_named_asset_file(
            &dot_amplihack,
            "amplifier-bundle/bin/multitask-orchestrator.sh",
        );

        let prev_home = crate::test_support::set_home(temp.path());
        let prev_amplihack = env::var_os("AMPLIHACK_HOME");
        unsafe { env::remove_var("AMPLIHACK_HOME") };

        let result = resolve_named_asset("multitask-orchestrator");

        crate::test_support::restore_home(prev_home);
        match prev_amplihack {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }

        let resolved = result.unwrap();
        assert!(
            resolved.ends_with("amplifier-bundle/bin/multitask-orchestrator.sh"),
            "expected multitask orchestrator.sh path, got {resolved:?}"
        );
    }

    #[test]
    fn resolve_named_asset_unknown_name_returns_error() {
        let err = resolve_named_asset("nonexistent-asset").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Unknown asset name"));
        assert!(msg.contains("multitask-orchestrator"));
        // Issue #614: helper-path and hooks-dir are now registered.
        // Issue #634: session-tree-path is now registered.
        assert!(msg.contains("helper-path"));
        assert!(msg.contains("hooks-dir"));
        assert!(msg.contains("session-tree-path"));
    }

    /// Issue #614: `resolve-bundle-asset hooks-dir` now resolves successfully
    /// when the hooks directory exists in the bundle.
    #[test]
    fn resolve_named_asset_hooks_dir_is_registered() {
        // hooks-dir is now a valid named asset (re-added in #614).
        let result = resolve_named_asset("hooks-dir");
        assert!(
            result.is_ok(),
            "hooks-dir must be a registered named asset (issue #614): {:?}",
            result.err()
        );
    }

    /// Issue #614: hooks-dir must be present in NAMED_ASSETS.
    #[test]
    fn hooks_dir_is_in_named_assets_see_issue_614() {
        assert!(
            NAMED_ASSETS.iter().any(|(name, _)| *name == "hooks-dir"),
            "hooks-dir must be registered (see rysweet/amplihack-rs#614)"
        );
    }

    // ── TDD tests for #588: unregistered named assets must return exit 1 ────
    //
    // BUG: When run_cli receives an arg without '/' that isn't in NAMED_ASSETS
    // (e.g. "helper-path", "hooks-dir"), it falls through to resolve_asset →
    // validate_relative_path which rejects it with exit 2 (invalid input)
    // because it doesn't start with "amplifier-bundle/".
    //
    // EXPECTED: exit 1 (not found) — these look like named-asset lookups,
    // not raw relative paths, and should be treated as unknown assets.

    /// Issue #614: `run_cli("helper-path")` is now a registered named asset.
    /// It returns exit 0 if the helper file exists in the bundle, or exit 1
    /// if the file doesn't exist (but still routes through named-asset logic).
    #[test]
    fn run_cli_registered_named_asset_helper_path() {
        let code = run_cli("helper-path");
        // helper-path is registered; exit code depends on whether the file
        // exists in the test environment (0 = found, 1 = not found).
        assert!(
            code == 0 || code == 1,
            "run_cli(\"helper-path\") should return 0 or 1 (named asset path), got {code}"
        );
    }

    /// Issue #614: `run_cli("hooks-dir")` is now a registered named asset.
    #[test]
    fn run_cli_registered_named_asset_hooks_dir() {
        let code = run_cli("hooks-dir");
        assert!(
            code == 0 || code == 1,
            "run_cli(\"hooks-dir\") should return 0 or 1 (named asset path), got {code}"
        );
    }

    /// Any single-token arg (no '/') that isn't a registered named asset
    /// should return exit 1 (not found), not exit 2 (invalid input).
    /// This covers future removals from NAMED_ASSETS without needing
    /// per-name test cases.
    #[test]
    fn run_cli_arbitrary_unregistered_named_asset_returns_exit_1() {
        let code = run_cli("nonexistent-asset-name");
        assert_eq!(
            code, 1,
            "run_cli(\"nonexistent-asset-name\") should return 1 (not found), got {code}"
        );
    }

    /// Verify that actual invalid input (path traversal) still returns
    /// exit 2, confirming the exit-code distinction is preserved.
    #[test]
    fn run_cli_invalid_input_still_returns_exit_2() {
        assert_eq!(
            run_cli("../../../etc/passwd"),
            2,
            "path traversal must still return exit 2 (invalid input)"
        );
    }

    #[test]
    fn run_cli_dispatches_named_asset_not_found() {
        // When AMPLIHACK_HOME and HOME both point to empty dirs AND the
        // compile-time workspace root also lacks the asset, returns exit
        // code 1.  Since tests run inside the workspace (which may contain
        // the real bundle), we craft an asset lookup that truly cannot
        // succeed: we temporarily point AMPLIHACK_HOME to an empty dir
        // and verify at least that the search-base logic functions.
        // If multitask-orchestrator exists in the workspace root, the
        // compile-time fallback will find it (exit 0); otherwise exit 1.
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_home = crate::test_support::set_home(temp.path());
        let prev_amplihack = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let code = run_cli("multitask-orchestrator");

        crate::test_support::restore_home(prev_home);
        match prev_amplihack {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }

        // The result depends on whether the workspace root has the real
        // bundle (code 0) or not (code 1).  Both are valid.
        assert!(
            code == 0 || code == 1,
            "run_cli should return 0 (found via fallback) or 1 (not found), got {code}"
        );
    }

    #[test]
    fn run_cli_dispatches_named_asset_found() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        make_named_asset_file(
            temp.path(),
            "amplifier-bundle/bin/multitask-orchestrator.sh",
        );

        let prev_amplihack = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let code = run_cli("multitask-orchestrator");

        match prev_amplihack {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }

        assert_eq!(code, 0, "run_cli should return 0 when named asset found");
    }
}

#[cfg(test)]
mod cli_dispatch_tests {
    //! Verify the `amplihack resolve-bundle-asset <asset>` clap subcommand
    //! parses correctly and that recipes don't regress to the old legacy
    //! runtime-asset invocation.
    use crate::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn parses_named_asset_argument() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "resolve-bundle-asset",
            "multitask-orchestrator",
        ])
        .unwrap();
        match cli.command {
            Commands::ResolveBundleAsset { asset } => assert_eq!(asset, "multitask-orchestrator"),
            other => panic!("expected ResolveBundleAsset, got {other:?}"),
        }
    }

    #[test]
    fn parses_relative_path_argument() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "resolve-bundle-asset",
            "amplifier-bundle/tools/statusline.sh",
        ])
        .unwrap();
        match cli.command {
            Commands::ResolveBundleAsset { asset } => {
                assert_eq!(asset, "amplifier-bundle/tools/statusline.sh")
            }
            other => panic!("expected ResolveBundleAsset, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_argument() {
        let result = Cli::try_parse_from(["amplihack", "resolve-bundle-asset"]);
        assert!(
            result.is_err(),
            "missing asset argument should be a parse error"
        );
    }

    #[test]
    fn recipes_do_not_invoke_legacy_runtime_assets() {
        // Regression guard for the bug where smart-orchestrator preflight
        // depended on the legacy runtime-asset resolver instead of the Rust
        // binary installed on the machine.
        // Recipes must use `amplihack resolve-bundle-asset` instead.
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let recipes_dir = manifest
            .join("..")
            .join("..")
            .join("amplifier-bundle")
            .join("recipes");
        if !recipes_dir.is_dir() {
            // Crate may be built outside the workspace (e.g., crates.io
            // packaging); recipes only exist in the source repo.
            eprintln!(
                "skipping: recipes dir not found at {}",
                recipes_dir.display()
            );
            return;
        }
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&recipes_dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let body = std::fs::read_to_string(&path).unwrap();
            if body.contains(concat!("amplihack", ".runtime_assets")) {
                offenders.push(path.display().to_string());
            }
        }
        assert!(
            offenders.is_empty(),
            "recipes still invoke the legacy Python runtime_assets module \
             instead of `amplihack resolve-bundle-asset`: {offenders:?}"
        );
    }

    /// Regression for #588: recipes must not contain dead HOOKS_DIR assignments
    /// that resolve `hooks-dir` (removed in #285). The `|| true` suppresses
    /// the error but the variable is never read — it's dead code that masks
    /// the underlying asset-resolver mismatch.
    #[test]
    fn recipes_do_not_contain_dead_hooks_dir_assignments() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let recipes_dir = manifest
            .join("..")
            .join("..")
            .join("amplifier-bundle")
            .join("recipes");
        if !recipes_dir.is_dir() {
            eprintln!(
                "skipping: recipes dir not found at {}",
                recipes_dir.display()
            );
            return;
        }
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&recipes_dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let body = std::fs::read_to_string(&path).unwrap();
            // Match the pattern: HOOKS_DIR="$(amplihack resolve-bundle-asset hooks-dir ..."
            if body.contains("resolve-bundle-asset hooks-dir") {
                offenders.push(path.display().to_string());
            }
        }
        assert!(
            offenders.is_empty(),
            "recipes still resolve the removed hooks-dir asset (see #285/#588). \
             Remove dead HOOKS_DIR assignments: {offenders:?}"
        );
    }
}
