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
    fs::create_dir_all(dest)?;
    let mut count = 0;

    for file in walk_files(source)? {
        if file.extension().is_some_and(|ext| ext == "md") {
            let relative = file.strip_prefix(source).unwrap_or(&file);
            let flat_name = relative.to_string_lossy().replace(['/', '\\'], "_");
            let target = dest.join(flat_name);
            fs::copy(&file, &target)
                .with_context(|| format!("copy {} → {}", file.display(), target.display()))?;
            count += 1;
        }
    }

    Ok(count)
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
