//! Trusted repository roots and private runtime directory creation.
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Trusted operator context: current repository plus explicitly supplied repository roots.
/// Never resolve model-provided paths or search the user's home directory.
pub fn repositories() -> Result<BTreeMap<String, PathBuf>> {
    let root = std::env::var_os("AMPLIHACK_REPO_PATH")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let mut roots: Vec<PathBuf> = vec![root];
    if let Some(value) = std::env::var_os("AMPLIHACK_REPOSITORY_ROOTS") {
        roots.extend(std::env::split_paths(&value));
    }
    let mut result = BTreeMap::new();
    for root in roots {
        let root = root
            .canonicalize()
            .with_context(|| format!("trusted repository {}", root.display()))?;
        let mut cmd = amplihack_git::command();
        let output = cmd
            .args(["remote", "get-url", "origin"])
            .current_dir(&root)
            .output()?;
        ensure!(
            output.status.success(),
            "trusted repository {} has no origin",
            root.display()
        );
        let remote = String::from_utf8(output.stdout)?;
        let name = amplihack_workflows::remote_repository::github_repository_from_remote_url(
            remote.trim(),
        )
        .unwrap_or_else(|| root.to_string_lossy().into_owned());
        if let Some(previous) = result.insert(name.clone(), root.clone()) {
            ensure!(previous == root, "ambiguous trusted repository {name}");
        }
    }
    Ok(result)
}

pub fn repository_names(roots: &BTreeMap<String, PathBuf>) -> Vec<String> {
    // Preserve the current repository as the legacy default.
    let current = std::env::var_os("AMPLIHACK_REPO_PATH")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let current = current.and_then(|p| p.canonicalize().ok());
    let mut names: Vec<_> = roots.keys().cloned().collect();
    names.sort_by_key(|k| roots.get(k) != current.as_ref());
    names
}

pub fn private_directory(path: &Path) -> Result<()> {
    ensure!(
        !path.is_symlink(),
        "private directory is a symlink: {}",
        path.display()
    );
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
