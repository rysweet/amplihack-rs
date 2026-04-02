use crate::commands::memory::{ensure_parent_dir, memory_home_paths, parse_json_value};
use anyhow::Result;
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum directory depth to prevent unbounded recursion.
const MAX_DIR_DEPTH: usize = 64;

pub(crate) struct HierarchicalMemoryPaths {
    pub(super) graph_base: PathBuf,
    pub(super) sqlite_db: PathBuf,
}

impl HierarchicalMemoryPaths {
    pub(super) fn neutral_graph_db(&self) -> PathBuf {
        self.graph_base.join("graph_db")
    }

    pub(super) fn legacy_graph_db(&self) -> PathBuf {
        self.graph_base.join("kuzu_db")
    }

    pub(super) fn resolved_graph_db(&self) -> PathBuf {
        if matches!(
            self.graph_base.file_name().and_then(|name| name.to_str()),
            Some("graph_db" | "kuzu_db")
        ) || self.graph_base.join("kuzu.lock").exists()
        {
            return self.graph_base.clone();
        }

        if self.graph_base.is_dir() && !self.graph_base.join("kuzu.lock").exists() {
            let neutral = self.neutral_graph_db();
            let legacy = self.legacy_graph_db();
            // Prefer legacy kuzu_db when it exists AND neutral is absent or empty.
            // An auto-created empty graph_db directory (from a prior failed resolve) must
            // not override a populated kuzu_db — that is the regression this condition fixes.
            if legacy.exists() && (!neutral.exists() || is_dir_empty(&neutral)) {
                return legacy;
            }
            return neutral;
        }

        let neutral = self.neutral_graph_db();
        if neutral.exists() {
            return neutral;
        }

        let legacy = self.legacy_graph_db();
        if legacy.exists() {
            return legacy;
        }

        self.graph_base.clone()
    }
}

/// Returns `true` if `path` is a directory that contains no entries.
/// Returns `false` for non-existent paths, files, or directories with any content.
pub(crate) fn is_dir_empty(path: &Path) -> bool {
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

pub(crate) fn resolve_hierarchical_memory_paths(
    agent_name: &str,
    storage_path: Option<&str>,
) -> Result<HierarchicalMemoryPaths> {
    super::sqlite_backend::validate_agent_name(agent_name)?;
    let storage_root = match storage_path {
        Some(path) => PathBuf::from(path),
        None => memory_home_paths()?.hierarchical_memory_dir,
    };
    let graph_base = match storage_path {
        Some(_) => storage_root.clone(),
        None => storage_root.join(agent_name),
    };
    let sqlite_db = storage_root.join(format!("{agent_name}.db"));
    Ok(HierarchicalMemoryPaths {
        graph_base,
        sqlite_db,
    })
}

pub(crate) fn resolve_hierarchical_db_path(
    agent_name: &str,
    storage_path: Option<&str>,
) -> Result<PathBuf> {
    Ok(resolve_hierarchical_memory_paths(agent_name, storage_path)?.resolved_graph_db())
}

pub(crate) fn copy_hierarchical_storage(src: &Path, dst: &Path) -> Result<()> {
    use anyhow::Context;
    // Use symlink_metadata (not is_dir) so we never silently follow
    // an attacker-placed symlink. Reject symlinks outright.
    let src_meta = src
        .symlink_metadata()
        .with_context(|| format!("failed to read metadata for {}", src.display()))?;
    if src_meta.file_type().is_symlink() {
        anyhow::bail!(
            "Refusing to copy symlink at {}: symlink traversal rejected",
            src.display()
        );
    }
    if src_meta.is_dir() {
        copy_dir_recursive(src, dst)?;
        return Ok(());
    }
    fs::copy(src, dst)
        .with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    let mut seen = HashSet::new();
    copy_dir_recursive_inner(src, dst, 0, &mut seen)
}

fn copy_dir_recursive_inner(
    src: &Path,
    dst: &Path,
    depth: usize,
    seen: &mut HashSet<PathBuf>,
) -> Result<()> {
    if depth > MAX_DIR_DEPTH {
        anyhow::bail!(
            "copy_dir_recursive exceeded maximum depth ({MAX_DIR_DEPTH}) at {}",
            src.display()
        );
    }
    fs::create_dir_all(dst)?;
    let canonical = src.canonicalize().unwrap_or_else(|_| src.to_path_buf());
    if !seen.insert(canonical) {
        anyhow::bail!("symlink cycle detected at {}", src.display());
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if kind.is_symlink() {
            // Skip symlinks with a warning to prevent directory traversal attacks
            println!("  Skipping symlink: {}", from.display());
            continue;
        } else if kind.is_dir() {
            copy_dir_recursive_inner(&from, &to, depth + 1, seen)?;
        } else if kind.is_file() {
            ensure_parent_dir(&to)?;
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

pub(crate) fn compute_path_size(path: &Path) -> Result<u64> {
    compute_path_size_inner(path, 0)
}

fn compute_path_size_inner(path: &Path, depth: usize) -> Result<u64> {
    if depth > MAX_DIR_DEPTH {
        anyhow::bail!(
            "compute_path_size exceeded maximum depth ({MAX_DIR_DEPTH}) at {}",
            path.display()
        );
    }
    if path.is_file() {
        return Ok(path.metadata()?.len());
    }
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        total += compute_path_size_inner(&entry.path(), depth + 1)?;
    }
    Ok(total)
}

pub(crate) fn graph_export_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Match Python well enough for parity comparisons that normalize timestamps.
    now.to_string()
}

pub(crate) fn parse_json_array_of_strings(value: &str) -> Result<Vec<String>> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let parsed = parse_json_value(value)?;
    match parsed {
        JsonValue::Array(items) => Ok(items
            .into_iter()
            .filter_map(|item| match item {
                JsonValue::String(value) => Some(value),
                _ => None,
            })
            .collect()),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn copy_hierarchical_storage_rejects_symlink_source() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real_file");
        std::fs::write(&real, "data").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let dst = dir.path().join("dst");

        let err = copy_hierarchical_storage(&link, &dst).unwrap_err();
        assert!(
            err.to_string().contains("symlink traversal rejected"),
            "expected symlink rejection, got: {err}"
        );
    }

    #[test]
    fn copy_hierarchical_storage_copies_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src_file");
        std::fs::write(&src, "hello").unwrap();
        let dst = dir.path().join("dst_file");

        copy_hierarchical_storage(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello");
    }

    #[test]
    fn copy_hierarchical_storage_copies_directory() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src_dir");
        std::fs::create_dir(&src_dir).unwrap();
        std::fs::write(src_dir.join("a.txt"), "aaa").unwrap();
        let dst_dir = dir.path().join("dst_dir");

        copy_hierarchical_storage(&src_dir, &dst_dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst_dir.join("a.txt")).unwrap(),
            "aaa"
        );
    }
}
