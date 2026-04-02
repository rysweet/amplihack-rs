//! Directory creation, tree copying, and framework asset initialization.

use super::filesystem::*;
use super::types::*;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn ensure_dirs(claude_dir: &Path) -> Result<()> {
    fs::create_dir_all(claude_dir)
        .with_context(|| format!("failed to create {}", claude_dir.display()))
}

pub(super) fn find_source_claude_dir(repo_root: &Path) -> Result<PathBuf> {
    let direct = repo_root.join(".claude");
    if direct.exists() {
        return Ok(direct);
    }
    let parent = repo_root.join("..").join(".claude");
    if parent.exists() {
        return Ok(parent);
    }
    anyhow::bail!(
        ".claude not found at {} or {}",
        direct.display(),
        parent.display()
    )
}

pub(super) fn copytree_manifest(source_claude: &Path, claude_dir: &Path) -> Result<Vec<String>> {
    let mut copied = Vec::new();
    for dir in ESSENTIAL_DIRS {
        let source_dir = source_claude.join(dir);
        if !source_dir.exists() {
            println!("  ⚠️  Warning: {dir} not found in source, skipping");
            continue;
        }

        let target_dir = claude_dir.join(dir);
        if let Some(parent) = target_dir.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        copy_dir_recursive(&source_dir, &target_dir)?;
        if dir.starts_with("tools/") {
            let files_updated = set_hook_permissions(&target_dir)?;
            if files_updated > 0 {
                println!("  🔐 Set execute permissions on {files_updated} hook files");
            }
        }
        println!("  ✅ Copied {dir}");
        copied.push((*dir).to_string());
    }

    let removed_legacy_hooks = prune_legacy_amplihack_hook_assets(claude_dir)?;
    if removed_legacy_hooks > 0 {
        println!(
            "  🧹 Removed {removed_legacy_hooks} legacy Python hook asset(s) from staged amplihack tools"
        );
    }

    let settings_src = source_claude.join("settings.json");
    let settings_dst = claude_dir.join("settings.json");
    if settings_src.exists() && !settings_dst.exists() {
        fs::copy(&settings_src, &settings_dst).with_context(|| {
            format!(
                "failed to copy {} to {}",
                settings_src.display(),
                settings_dst.display()
            )
        })?;
        println!("  ✅ Copied settings.json");
    }

    for file in ESSENTIAL_FILES {
        let source_file = source_claude.join(file);
        if !source_file.exists() {
            println!("  ⚠️  Warning: {file} not found in source, skipping");
            continue;
        }
        let target_file = claude_dir.join(file);
        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(&source_file, &target_file).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_file.display(),
                target_file.display()
            )
        })?;
        set_script_permissions(&target_file)?;
        println!("  ✅ Copied {file}");
    }

    let source_claude_md = source_claude
        .parent()
        .context("source .claude dir missing parent")?
        .join("CLAUDE.md");
    if source_claude_md.exists() {
        let target_claude_md = claude_dir
            .parent()
            .context("target .claude dir missing parent")?
            .join("CLAUDE.md");
        fs::copy(&source_claude_md, &target_claude_md).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_claude_md.display(),
                target_claude_md.display()
            )
        })?;
        println!("  ✅ Installed amplihack CLAUDE.md");
    }

    Ok(copied)
}

pub(super) fn prune_legacy_amplihack_hook_assets(claude_dir: &Path) -> Result<usize> {
    let hooks_dir = claude_dir.join("tools").join("amplihack").join("hooks");
    if !hooks_dir.exists() {
        return Ok(0);
    }

    let mut removed = 0;
    for entry in fs::read_dir(&hooks_dir)
        .with_context(|| format!("failed to read {}", hooks_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || path.extension().and_then(|ext| ext.to_str()) != Some("py")
        {
            continue;
        }
        fs::remove_file(&path).with_context(|| format!("failed to remove {}", path.display()))?;
        removed += 1;
    }

    if removed > 0 && fs::read_dir(&hooks_dir)?.next().is_none() {
        fs::remove_dir(&hooks_dir)
            .with_context(|| format!("failed to remove {}", hooks_dir.display()))?;
    }

    Ok(removed)
}

pub(super) fn initialize_project_md(claude_dir: &Path) -> Result<()> {
    let project_md = claude_dir.join("context").join("PROJECT.md");
    if let Some(parent) = project_md.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(
        &project_md,
        "# Project Overview\n\nThis PROJECT.md was initialized by amplihack.\n",
    )
    .with_context(|| format!("failed to write {}", project_md.display()))?;
    println!("   ✅ PROJECT.md initialized using template");
    Ok(())
}

pub(super) fn create_runtime_dirs(claude_dir: &Path) -> Result<()> {
    for dir in RUNTIME_DIRS {
        let full = claude_dir.join(dir);
        fs::create_dir_all(&full)
            .with_context(|| format!("failed to create {}", full.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&full, std::fs::Permissions::from_mode(0o755))
                .with_context(|| format!("failed to set permissions on {}", full.display()))?;
        }
        println!("  ✅ Runtime directory {dir} ready");
    }
    Ok(())
}
