//! Filesystem helpers — directory reset, tree flattening, recursive copy.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn reset_markdown_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                fs::remove_file(&path)?;
            }
        }
    } else {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

pub(super) fn flatten_markdown_tree(source: &Path, dest: &Path) -> Result<usize> {
    let mut copied = 0usize;
    for path in walk_files(source)? {
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let file_name = path
            .file_name()
            .context("source markdown file missing name")?;
        fs::copy(&path, dest.join(file_name)).with_context(|| {
            format!("failed to copy {} into {}", path.display(), dest.display())
        })?;
        copied += 1;
    }
    Ok(copied)
}

pub(super) fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    if !root.is_dir() {
        return Ok(result);
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            result.extend(walk_files(&path)?);
        } else if path.is_file() {
            result.push(path);
        }
    }
    Ok(result)
}

#[allow(dead_code)] // Utility kept for staging operations
pub(super) fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = dest.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if source_path.is_file() {
            fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}
