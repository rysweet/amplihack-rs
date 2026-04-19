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

/// Stage the `amplifier-bundle/` tree from the source repo into
/// `~/.amplihack/amplifier-bundle/`.
///
/// The dev-orchestrator skill's mandatory execution path
/// (`amplihack recipe run smart-orchestrator`) is unreachable without these
/// recipes (`smart-orchestrator.yaml`, `default-workflow.yaml`,
/// `investigation-workflow.yaml`) and the `tools/orch_helper.py` referenced
/// by the parse-decomposition step. See issue #243.
///
/// The bundle is copied to `~/.amplihack/amplifier-bundle/` so the recipe
/// runner's `AMPLIHACK_HOME/amplifier-bundle/recipes` lookup (and the
/// skill's `AMPLIHACK_HOME` auto-detection that walks for an
/// `amplifier-bundle/` folder) both succeed.
///
/// Returns `Ok(true)` if the bundle was staged. Returns an error if the
/// source repo lacks an `amplifier-bundle/` directory, since
/// [`super::settings::missing_framework_paths`] treats the bundle's recipes
/// and `tools/orch_helper.py` as required framework assets — a missing
/// source bundle would cause every subsequent launcher boot to attempt
/// (and fail) a re-install in a tight loop.
///
/// The copy is performed via a temp-dir + atomic-rename pattern so a failed
/// mid-flight copy never destroys an existing working bundle.
///
/// The source `amplifier-bundle/` root must be a real directory; symlinked
/// roots are rejected to prevent a malicious local repo from copying an
/// arbitrary readable directory into the user's staging area.
pub(super) fn copy_amplifier_bundle(repo_root: &Path, claude_dir: &Path) -> Result<bool> {
    let source_bundle = repo_root.join("amplifier-bundle");
    let source_meta = fs::symlink_metadata(&source_bundle).with_context(|| {
        format!(
            "amplifier-bundle not found at {} — required for dev-orchestrator \
             recipe execution (#243)",
            source_bundle.display()
        )
    })?;
    if source_meta.file_type().is_symlink() {
        anyhow::bail!(
            "refusing to stage amplifier-bundle from symlinked source root at {} \
             — bundle root must be a real directory (#243)",
            source_bundle.display()
        );
    }
    if !source_meta.is_dir() {
        anyhow::bail!(
            "amplifier-bundle source at {} is not a directory",
            source_bundle.display()
        );
    }

    let target_bundle = claude_dir
        .parent()
        .context("staging .claude dir missing parent")?
        .join("amplifier-bundle");
    if let Some(parent) = target_bundle.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    // Atomic replacement: copy into sibling temp dir first, then swap.
    // If the copy fails, the existing bundle remains intact — this matters
    // because `ensure_framework_installed` swallows refresh errors and
    // expects the previous staged framework to still be usable.
    let staging_temp = target_bundle.with_extension("staging");
    if staging_temp.exists() {
        fs::remove_dir_all(&staging_temp).with_context(|| {
            format!(
                "failed to clean stale staging dir {}",
                staging_temp.display()
            )
        })?;
    }
    copy_dir_recursive(&source_bundle, &staging_temp).with_context(|| {
        format!(
            "failed to copy amplifier-bundle into staging dir {}",
            staging_temp.display()
        )
    })?;

    if target_bundle.exists() {
        let backup = target_bundle.with_extension("old");
        if backup.exists() {
            fs::remove_dir_all(&backup).ok();
        }
        fs::rename(&target_bundle, &backup).with_context(|| {
            format!(
                "failed to back up existing amplifier-bundle at {}",
                target_bundle.display()
            )
        })?;
        if let Err(err) = fs::rename(&staging_temp, &target_bundle) {
            // Roll the previous bundle back into place so the install isn't bricked.
            let _ = fs::rename(&backup, &target_bundle);
            return Err(err).with_context(|| {
                format!(
                    "failed to swap new amplifier-bundle into {}",
                    target_bundle.display()
                )
            });
        }
        let _ = fs::remove_dir_all(&backup);
    } else {
        fs::rename(&staging_temp, &target_bundle).with_context(|| {
            format!(
                "failed to move new amplifier-bundle into {}",
                target_bundle.display()
            )
        })?;
    }

    println!(
        "  ✅ Staged amplifier-bundle to {}",
        target_bundle.display()
    );
    Ok(true)
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
