use anyhow::{Result, bail};
use std::env;
use std::path::{Path, PathBuf};

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

fn search_bases() -> Vec<PathBuf> {
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
fn named_asset_search_bases() -> Vec<PathBuf> {
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
        let temp = tempfile::tempdir().unwrap();
        let asset = temp.path().join("amplifier-bundle/tools/orch_helper.py");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, "ok").unwrap();

        let prev_home = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let resolved = resolve_asset("amplifier-bundle/tools/orch_helper.py").unwrap();

        match prev_home {
            Some(value) => unsafe { env::set_var("AMPLIHACK_HOME", value) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }

        assert_eq!(resolved, asset.canonicalize().unwrap());
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
        let asset = make_named_asset_file(temp.path(), "amplifier-bundle/tools/orch_helper.py");

        let prev = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let result = resolve_named_asset("helper-path");

        match prev {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }

        assert_eq!(result.unwrap(), asset.canonicalize().unwrap());
    }

    #[test]
    fn resolve_named_asset_session_tree_path_uses_amplihack_home() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let asset = make_named_asset_file(temp.path(), "amplifier-bundle/tools/session_tree.py");

        let prev = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let result = resolve_named_asset("session-tree-path");

        match prev {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }

        assert_eq!(result.unwrap(), asset.canonicalize().unwrap());
    }

    #[test]
    fn resolve_named_asset_hooks_dir_uses_amplihack_home() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let asset = make_named_asset_dir(temp.path(), ".claude/tools/amplihack/hooks");

        let prev = env::var_os("AMPLIHACK_HOME");
        unsafe { env::set_var("AMPLIHACK_HOME", temp.path()) };

        let result = resolve_named_asset("hooks-dir");

        match prev {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }

        assert_eq!(result.unwrap(), asset.canonicalize().unwrap());
    }

    #[test]
    fn resolve_named_asset_falls_back_to_dot_amplihack() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let dot_amplihack = temp.path().join(".amplihack");
        let asset = make_named_asset_file(&dot_amplihack, "amplifier-bundle/tools/orch_helper.py");

        let prev_home = crate::test_support::set_home(temp.path());
        let prev_amplihack = env::var_os("AMPLIHACK_HOME");
        unsafe { env::remove_var("AMPLIHACK_HOME") };

        let result = resolve_named_asset("helper-path");

        crate::test_support::restore_home(prev_home);
        match prev_amplihack {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }

        assert_eq!(result.unwrap(), asset.canonicalize().unwrap());
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
        // With no AMPLIHACK_HOME set to a dir with the asset, returns exit code 1
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let prev_home = crate::test_support::set_home(temp.path());
        let prev_amplihack = env::var_os("AMPLIHACK_HOME");
        unsafe { env::remove_var("AMPLIHACK_HOME") };

        let code = run_cli("helper-path");

        crate::test_support::restore_home(prev_home);
        match prev_amplihack {
            Some(v) => unsafe { env::set_var("AMPLIHACK_HOME", v) },
            None => unsafe { env::remove_var("AMPLIHACK_HOME") },
        }

        assert_eq!(
            code, 1,
            "run_cli should return 1 when named asset not found"
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
