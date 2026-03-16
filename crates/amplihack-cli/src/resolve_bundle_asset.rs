use anyhow::{Result, bail};
use std::env;
use std::path::{Path, PathBuf};

const SAFE_PATH_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-./";

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

pub fn run_cli(relative_path: &str) -> i32 {
    match resolve_asset(relative_path) {
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
}
