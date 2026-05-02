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

/// Symlink-aware "is this a real directory?" probe used by `find_source_root`.
/// Refuses symlinked roots so a malicious local repo cannot make install copy
/// from an arbitrary readable directory (defense-in-depth, mirrors
/// `copy_amplifier_bundle`).
fn is_real_dir(p: &Path) -> bool {
    match fs::symlink_metadata(p) {
        Ok(md) => md.is_dir() && !md.file_type().is_symlink(),
        Err(_) => false,
    }
}

/// Probe the supplied repo root for a usable framework-asset source layout.
/// Resolution order (per design):
///   1. `<root>/amplifier-bundle/`  → [`SourceLayout::Bundle`]
///   2. `<root>/.claude/`           → [`SourceLayout::LegacyClaude`]
///   3. `<root>/../.claude/`        → [`SourceLayout::LegacyClaude`]
///
/// Symlinked roots are rejected at this layer (TOCTOU-defended again per-entry
/// inside `copy_dir_recursive`). On no match, bails with a diagnostic naming
/// every probed path so the user can fix the layout.
pub(super) fn find_source_root(repo_root: &Path) -> Result<(PathBuf, SourceLayout)> {
    let bundle = repo_root.join("amplifier-bundle");
    if is_real_dir(&bundle) {
        return Ok((bundle, SourceLayout::Bundle));
    }
    if bundle.exists() {
        // Exists but not a real dir (symlink, file). Fail loud.
        anyhow::bail!(
            "amplifier-bundle at {} is not a real directory (symlinks are rejected for safety)",
            bundle.display()
        );
    }
    let direct = repo_root.join(".claude");
    if is_real_dir(&direct) {
        return Ok((direct, SourceLayout::LegacyClaude));
    }
    let parent = repo_root.join("..").join(".claude");
    if is_real_dir(&parent) {
        return Ok((parent, SourceLayout::LegacyClaude));
    }
    anyhow::bail!(
        "no framework asset source found — searched: {}, {}, {}. \
         A valid amplihack-rs checkout must contain an `amplifier-bundle/` directory; \
         legacy installs may use `.claude/` instead.",
        bundle.display(),
        direct.display(),
        parent.display()
    )
}

/// Copy framework asset directories from `source_root` into `claude_dir` using
/// the supplied [`SourceLayout`]'s mapping table. Returns the destination-relative
/// directory names that were successfully copied (suitable for diff against
/// [`essential_destinations`]).
pub(super) fn copytree_manifest(
    source_root: &Path,
    layout: SourceLayout,
    claude_dir: &Path,
) -> Result<Vec<String>> {
    let mut copied = Vec::new();
    for (src_rel, dst_rel) in dir_mapping(layout) {
        // Defense in depth — the compile-time drift test forbids `..` in
        // mapping entries, but runtime check here keeps the invariant true
        // even if a future refactor goes around the table.
        for comp in Path::new(dst_rel).components() {
            if matches!(comp, std::path::Component::ParentDir) {
                anyhow::bail!(
                    "internal error: dst_rel `{dst_rel}` contains `..` — refusing to copy"
                );
            }
        }

        let source_dir = source_root.join(src_rel);
        if !source_dir.exists() {
            println!("  ⚠️  Warning: {src_rel} not found in source, skipping");
            continue;
        }

        let target_dir = claude_dir.join(dst_rel);
        if let Some(parent) = target_dir.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        copy_dir_recursive(&source_dir, &target_dir)?;
        if dst_rel.starts_with("tools/") {
            let files_updated = set_hook_permissions(&target_dir)?;
            if files_updated > 0 {
                println!("  🔐 Set execute permissions on {files_updated} hook files");
            }
        }
        println!("  ✅ Copied {src_rel} -> {dst_rel}");
        copied.push((*dst_rel).to_string());
    }

    // Issue #505: amplifier-bundle/tools/amplihack/hooks/ now intentionally
    // ships Python shim scripts that delegate to the native amplihack-hooks
    // binary. They satisfy consumers (recipe-runner, settings.json templates)
    // that invoke hooks by absolute path. Pruning them post-copy would
    // immediately undo the staging we just performed, so the pre-#505
    // `prune_legacy_amplihack_hook_assets` call has been removed. Leftover
    // pre-#505 Python hooks from earlier installs are harmless: they get
    // overwritten by the fresh shims via copy_dir_recursive above.

    let settings_src = source_root.join("settings.json");
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

    for file in essential_files(layout) {
        let source_file = source_root.join(file);
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

    // CLAUDE.md staging — issue #527.
    //
    // Pre-fix: only `source_root.parent()/CLAUDE.md` was probed and a
    // missing source was silently skipped. With the bundle layout, the
    // canonical CLAUDE.md ships at `<repo>/amplifier-bundle/CLAUDE.md`
    // (i.e., inside `source_root`), not at the repo root, so installs from
    // a bundle-only checkout left `$AMPLIHACK_HOME/CLAUDE.md` absent and
    // the verifier reported ❌. Per the install-completeness invariant in
    // amplifier-bundle/context/PHILOSOPHY.md, we must (a) probe the bundle
    // path first, (b) fall back to the legacy repo-root path, and (c) bail
    // loudly when neither exists.
    let parent_dir = source_root
        .parent()
        .context("source root missing parent for CLAUDE.md lookup")?;
    let bundle_candidate = source_root.join("CLAUDE.md");
    let legacy_candidate = parent_dir.join("CLAUDE.md");
    let chosen_source = if bundle_candidate.exists() {
        bundle_candidate.clone()
    } else if legacy_candidate.exists() {
        legacy_candidate.clone()
    } else {
        anyhow::bail!(
            "CLAUDE.md not found in source repo. Searched: {} and {}. \
             A valid amplihack-rs checkout must ship CLAUDE.md inside \
             `amplifier-bundle/CLAUDE.md` (preferred, bundle layout) or at \
             the repo root (legacy layout). Without CLAUDE.md the install \
             verifier will report `$AMPLIHACK_HOME/CLAUDE.md` missing.",
            bundle_candidate.display(),
            legacy_candidate.display()
        );
    };
    let target_claude_md = claude_dir
        .parent()
        .context("target .claude dir missing parent")?
        .join("CLAUDE.md");
    if let Some(parent) = target_claude_md.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::copy(&chosen_source, &target_claude_md).with_context(|| {
        format!(
            "failed to copy {} to {}",
            chosen_source.display(),
            target_claude_md.display()
        )
    })?;
    println!(
        "  ✅ Installed amplihack CLAUDE.md (from {})",
        chosen_source.display()
    );

    Ok(copied)
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
