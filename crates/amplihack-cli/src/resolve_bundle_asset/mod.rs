use anyhow::{Result, bail};
#[cfg(test)]
use std::env;
use std::path::{Path, PathBuf};

mod search;

use search::{named_asset_search_bases, search_bases};

const SAFE_PATH_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-./";

/// Named asset mappings matching Python's runtime_assets._ASSET_RELATIVE_PATHS.
///
/// Each entry is `(name, &[relative_paths])` where relative_paths are tried in order.
const NAMED_ASSETS: &[(&str, &[&str])] = &[
    ("helper-path", &["amplifier-bundle/tools/orch_helper.py"]),
    (
        "session-tree-path",
        &["amplifier-bundle/tools/session_tree.py"],
    ),
    (
        "hooks-dir",
        &[
            ".claude/tools/amplihack/hooks",
            "amplifier-bundle/tools/amplihack/hooks",
        ],
    ),
    // FIX (rysweet/amplihack-rs#283/#248): expose the multitask-orchestrator
    // script under a stable named-asset key so smart-orchestrator's
    // launch-parallel-round-1 step can resolve it via the Rust CLI instead of
    // the legacy `python3 -m amplihack.runtime_assets multitask-orchestrator`
    // shim. The candidate paths mirror those in `runtime_assets::asset_relative_paths`.
    (
        "multitask-orchestrator",
        &[
            ".claude/skills/multitask/orchestrator.py",
            "amplifier-bundle/skills/multitask/orchestrator.py",
        ],
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
    if !relative_path.chars().all(|ch| SAFE_PATH_CHARS.contains(ch)) {
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
/// Named assets are logical aliases defined in [`NAMED_ASSETS`].
/// Each name expands to one or more candidate relative paths that are tried
/// in order against each search base.
///
/// Search priority matches Python's `iter_runtime_roots()`:
/// 1. `AMPLIHACK_HOME` env var
/// 2. `~/.amplihack`
/// 3. Walk up from cwd for a repo/project root marker
/// 4. Workspace root (compile-time anchor)
/// 5. cwd
pub fn resolve_named_asset(name: &str) -> Result<PathBuf> {
    let rel_paths = NAMED_ASSETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, paths)| *paths)
        .ok_or_else(|| {
            let valid = NAMED_ASSETS
                .iter()
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::anyhow!("Unknown asset name {name:?}. Expected one of: {valid}")
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

pub fn run_cli(arg: &str) -> i32 {
    // Dispatch named assets (e.g. "helper-path", "session-tree-path", "hooks-dir")
    if NAMED_ASSETS.iter().any(|(name, _)| *name == arg) {
        return match resolve_named_asset(arg) {
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
        };
    }

    // Fall through to raw amplifier-bundle/ path resolution
    match resolve_asset(arg) {
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
        let err = validate_relative_path("tools/orch_helper.py").unwrap_err();
        assert!(
            err.to_string()
                .contains("must start with 'amplifier-bundle/'")
        );
    }

    #[test]
    fn validate_accepts_normal_bundle_path() {
        validate_relative_path("amplifier-bundle/tools/orch_helper.py").unwrap();
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
        let asset = temp.path().join("amplifier-bundle/tools/orch_helper.py");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, "ok").unwrap();

        let prev_home = crate::test_support::set_home(temp.path());
        let prev_amplihack = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let resolved = resolve_asset("amplifier-bundle/tools/orch_helper.py").unwrap();

        match prev_amplihack {
            Some(value) => unsafe { env::set_var("AMPLIHACK_HOME", value) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }
        crate::test_support::restore_home(prev_home);

        assert!(
            resolved.ends_with("amplifier-bundle/tools/orch_helper.py"),
            "expected orch_helper.py path, got {:?}",
            resolved
        );
    }

    #[test]
    fn run_cli_returns_invalid_input_exit_code() {
        assert_eq!(run_cli("../../../etc/passwd"), 2);
    }

    // ── Named asset tests ─────────────────────────────────────────────────────

    fn make_named_asset_dir(base: &std::path::Path, rel_path: &str) -> std::path::PathBuf {
        let target = base.join(rel_path);
        std::fs::create_dir_all(&target).unwrap();
        target
    }

    fn make_named_asset_file(base: &std::path::Path, rel_path: &str) -> std::path::PathBuf {
        let target = base.join(rel_path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"ok").unwrap();
        target
    }

    #[test]
    fn resolve_named_asset_helper_path_uses_amplihack_home() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        make_named_asset_file(temp.path(), "amplifier-bundle/tools/orch_helper.py");

        let prev_home = crate::test_support::set_home(temp.path());
        let prev = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let result = resolve_named_asset("helper-path");

        match prev {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }
        crate::test_support::restore_home(prev_home);

        // Accept match at AMPLIHACK_HOME or any fallback (cwd/workspace root
        // may also contain the file after the bundle mirror).
        let resolved = result.unwrap();
        assert!(
            resolved.ends_with("amplifier-bundle/tools/orch_helper.py"),
            "expected orch_helper.py path, got {:?}",
            resolved
        );
    }

    #[test]
    fn resolve_named_asset_session_tree_path_uses_amplihack_home() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        make_named_asset_file(temp.path(), "amplifier-bundle/tools/session_tree.py");

        let prev_home = crate::test_support::set_home(temp.path());
        let prev = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let result = resolve_named_asset("session-tree-path");

        match prev {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }
        crate::test_support::restore_home(prev_home);

        let resolved = result.unwrap();
        assert!(
            resolved.ends_with("amplifier-bundle/tools/session_tree.py"),
            "expected session_tree.py path, got {:?}",
            resolved
        );
    }

    #[test]
    fn resolve_named_asset_multitask_orchestrator_uses_amplihack_home() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        make_named_asset_file(
            temp.path(),
            "amplifier-bundle/skills/multitask/orchestrator.py",
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
            resolved.ends_with("amplifier-bundle/skills/multitask/orchestrator.py"),
            "expected multitask orchestrator.py path, got {:?}",
            resolved
        );
    }

    #[test]
    fn run_cli_resolves_multitask_orchestrator_named_asset() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        make_named_asset_file(temp.path(), ".claude/skills/multitask/orchestrator.py");

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
    fn resolve_named_asset_hooks_dir_uses_amplihack_home() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        make_named_asset_dir(temp.path(), ".claude/tools/amplihack/hooks");

        let prev_home = crate::test_support::set_home(temp.path());
        let prev = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let result = resolve_named_asset("hooks-dir");

        match prev {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }
        crate::test_support::restore_home(prev_home);

        let resolved = result.unwrap();
        assert!(
            resolved.to_string_lossy().contains("hooks"),
            "expected hooks dir path, got {:?}",
            resolved
        );
    }

    #[test]
    fn resolve_named_asset_falls_back_to_dot_amplihack() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let dot_amplihack = temp.path().join(".amplihack");
        make_named_asset_file(&dot_amplihack, "amplifier-bundle/tools/orch_helper.py");

        let prev_home = crate::test_support::set_home(temp.path());
        let prev_amplihack = env::var_os("AMPLIHACK_HOME");
        unsafe { env::remove_var("AMPLIHACK_HOME") };

        let result = resolve_named_asset("helper-path");

        crate::test_support::restore_home(prev_home);
        match prev_amplihack {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }

        let resolved = result.unwrap();
        assert!(
            resolved.ends_with("amplifier-bundle/tools/orch_helper.py"),
            "expected orch_helper.py path, got {:?}",
            resolved
        );
    }

    #[test]
    fn resolve_named_asset_unknown_name_returns_error() {
        let err = resolve_named_asset("nonexistent-asset").unwrap_err();
        assert!(err.to_string().contains("Unknown asset name"));
        assert!(err.to_string().contains("helper-path"));
        assert!(err.to_string().contains("hooks-dir"));
    }

    #[test]
    fn run_cli_dispatches_named_asset_not_found() {
        // When AMPLIHACK_HOME and HOME both point to empty dirs AND the
        // compile-time workspace root also lacks the asset, returns exit
        // code 1.  Since tests run inside the workspace (which may contain
        // the real bundle), we craft an asset lookup that truly cannot
        // succeed: we temporarily point AMPLIHACK_HOME to an empty dir
        // and verify at least that the search-base logic functions.
        // If orch_helper.py exists in the workspace root, the compile-time
        // fallback will find it (exit 0); otherwise exit 1.
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_home = crate::test_support::set_home(temp.path());
        let prev_amplihack = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let code = run_cli("helper-path");

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
        make_named_asset_file(temp.path(), "amplifier-bundle/tools/orch_helper.py");

        let prev_amplihack = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let code = run_cli("helper-path");

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
    //! parses correctly and that recipes don't regress to the old
    //! `python3 -m amplihack.runtime_assets ...` invocation.
    use crate::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn parses_named_asset_argument() {
        let cli =
            Cli::try_parse_from(["amplihack", "resolve-bundle-asset", "helper-path"]).unwrap();
        match cli.command {
            Commands::ResolveBundleAsset { asset } => assert_eq!(asset, "helper-path"),
            other => panic!("expected ResolveBundleAsset, got {other:?}"),
        }
    }

    #[test]
    fn parses_relative_path_argument() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "resolve-bundle-asset",
            "amplifier-bundle/tools/orch_helper.py",
        ])
        .unwrap();
        match cli.command {
            Commands::ResolveBundleAsset { asset } => {
                assert_eq!(asset, "amplifier-bundle/tools/orch_helper.py")
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
    fn recipes_do_not_invoke_python_runtime_assets() {
        // Regression guard for the bug where smart-orchestrator preflight
        // failed because `python3 -m amplihack.runtime_assets` is not
        // available on machines that only have the Rust binary installed.
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
            if body.contains("python3 -m amplihack.runtime_assets") {
                offenders.push(path.display().to_string());
            }
        }
        assert!(
            offenders.is_empty(),
            "recipes still invoke the legacy Python runtime_assets module \
             instead of `amplihack resolve-bundle-asset`: {offenders:?}"
        );
    }
}
