//! Filesystem walking, copying, and permission utilities.

use anyhow::{Context, Result};
use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn all_rel_dirs(claude_dir: &Path) -> Result<BTreeSet<String>> {
    let mut result = BTreeSet::new();
    if !claude_dir.exists() {
        return Ok(result);
    }
    for path in walk_dirs(claude_dir)? {
        let rel = path
            .strip_prefix(claude_dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        result.insert(if rel.is_empty() { ".".to_string() } else { rel });
    }
    Ok(result)
}

pub(super) fn get_all_files_and_dirs(
    claude_dir: &Path,
    root_dirs: &[PathBuf],
) -> Result<(Vec<String>, Vec<String>)> {
    let mut files = Vec::new();
    let mut dirs = BTreeSet::new();
    for root in root_dirs {
        if !root.exists() {
            continue;
        }
        for entry in walk_all(root)? {
            let rel = entry
                .strip_prefix(claude_dir)
                .unwrap_or(&entry)
                .to_string_lossy()
                .replace('\\', "/");
            if entry.is_dir() {
                dirs.insert(rel);
            } else if entry.is_file() {
                files.push(rel);
            }
        }
    }
    files.sort();
    Ok((files, dirs.into_iter().collect()))
}

const MAX_WALK_DEPTH: usize = 64;

/// BFS directory walk with predicate-based inclusion, symlink guard, and depth limit.
///
/// Symlinks are never followed — entries identified as symlinks via `symlink_metadata()`
/// are silently skipped to prevent directory traversal attacks.
/// Traversal stops at `MAX_WALK_DEPTH` to guard against pathologically deep trees.
///
/// The `include` predicate receives each `DirEntry` and controls whether it appears
/// in the returned list.  Directories are always queued for traversal regardless of
/// whether `include` returns `true` for them.  The root itself is always included.
fn walk_bounded(root: &Path, include: impl Fn(&fs::DirEntry) -> bool) -> Result<Vec<PathBuf>> {
    let mut results = vec![root.to_path_buf()];
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));

    while let Some((dir, depth)) = queue.pop_front() {
        if depth >= MAX_WALK_DEPTH {
            // Silently skip entries beyond the depth limit rather than failing the
            // entire walk; the limit protects against symlink loops and untrusted trees.
            continue;
        }
        for entry in
            fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))?
        {
            let entry = entry?;
            // symlink_metadata() does not follow symlinks — use it to detect them.
            let meta = entry
                .path()
                .symlink_metadata()
                .with_context(|| format!("failed to stat {}", entry.path().display()))?;
            if meta.file_type().is_symlink() {
                continue; // never follow symlinks
            }
            if meta.is_dir() {
                queue.push_back((entry.path(), depth + 1));
            }
            if include(&entry) {
                results.push(entry.path());
            }
        }
    }
    Ok(results)
}

/// Return the root directory and all subdirectories (no files).
fn walk_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    // DirEntry::file_type() does not follow symlinks; symlinks are already
    // filtered out by walk_bounded, so this predicate safely identifies real dirs.
    walk_bounded(root, |e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
}

/// Return the root directory and all entries within it (files and directories).
pub(super) fn walk_all(root: &Path) -> Result<Vec<PathBuf>> {
    walk_bounded(root, |_| true)
}

/// Returns `true` for directory names that should be excluded from copy operations.
fn is_excluded_dir(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some("__pycache__" | ".pytest_cache" | "node_modules")
    )
}

/// Returns `true` for file extensions that should be excluded from copy operations.
fn is_excluded_file(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .map(|s| s.ends_with(".pyc") || s.ends_with(".pyo"))
        .unwrap_or(false)
}

/// Copy a directory recursively, skipping symlinks with a warning.
/// Device files, sockets, and FIFOs are skipped silently.
/// `__pycache__` directories and `.pyc`/`.pyo` files are excluded.
pub(super) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    // Same-path guard: bail if source and destination resolve to the same location.
    if let (Ok(src_canon), Ok(dst_canon)) = (src.canonicalize(), dst.canonicalize())
        && src_canon == dst_canon
    {
        anyhow::bail!(
            "source and destination are the same path: {}",
            src_canon.display()
        );
    }

    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let source = entry.path();
        let file_name = entry.file_name();
        let target = dst.join(&file_name);
        // Use entry.file_type() — symlink-safe (does not follow symlinks)
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            // Skip symlinks with a warning to prevent directory traversal attacks
            println!("  ⚠️  Skipping symlink: {}", source.display());
            continue;
        } else if kind.is_dir() {
            if is_excluded_dir(&file_name) {
                continue;
            }
            copy_dir_recursive(&source, &target)?;
        } else if kind.is_file() {
            if is_excluded_file(&file_name) {
                continue;
            }
            fs::copy(&source, &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
        }
        // Device files, sockets, FIFOs: silently skipped
    }
    Ok(())
}

pub(super) fn set_hook_permissions(target_dir: &Path) -> Result<usize> {
    let mut updated = 0usize;
    for path in walk_all(target_dir)? {
        if path.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("py")
            && path
                .parent()
                .and_then(|value| value.file_name())
                .and_then(|value| value.to_str())
                == Some("hooks")
        {
            set_script_permissions(&path)?;
            updated += 1;
        }
    }
    Ok(updated)
}

pub(super) fn set_script_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata =
            fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
        let mut perms = metadata.permissions();
        perms.set_mode(perms.mode() | 0o110);
        fs::set_permissions(path, perms)
            .with_context(|| format!("failed to chmod {}", path.display()))?;
    }
    Ok(())
}
