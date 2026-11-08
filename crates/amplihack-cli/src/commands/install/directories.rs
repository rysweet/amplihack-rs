//! Directory creation, tree copying, and framework asset initialization.

use super::bundle_compat::{
    validate_framework_bundle_compatibility, validate_staged_framework_bundle,
};
use super::bundle_manifest;
use super::filesystem::*;
use super::paths::sanitize_path;
use super::release_manifest::ReleaseManifest;
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

        let source_dir = required_source_dir(source_root, src_rel)?;

        copy_mapped_dir(&source_dir, src_rel, claude_dir, dst_rel)?;
        copied.push((*dst_rel).to_string());
    }

    if layout == SourceLayout::Bundle {
        for (src_rel, dst_rel) in SOURCE_CONDITIONAL_BUNDLE_DIR_MAPPING {
            let source_dir = source_root.join(src_rel);
            if !source_dir.exists() {
                continue;
            }
            let source_dir = required_source_dir(source_root, src_rel)?;
            copy_mapped_dir(&source_dir, src_rel, claude_dir, dst_rel)?;
            copied.push((*dst_rel).to_string());
        }
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

    for file in required_source_files(layout) {
        let source_file = source_root.join(file);
        if !source_file.exists() {
            anyhow::bail!(
                "required framework source file is missing: {} (expected at {})",
                file,
                source_file.display()
            );
        }
        if !source_file.is_file() {
            anyhow::bail!(
                "required framework source path is not a file: {} (at {})",
                file,
                source_file.display()
            );
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

fn required_source_dir(source_root: &Path, src_rel: &str) -> Result<PathBuf> {
    let source_dir = source_root.join(src_rel);
    let metadata = fs::symlink_metadata(&source_dir).with_context(|| {
        format!(
            "required framework source directory is missing: {} (expected at {})",
            src_rel,
            source_dir.display()
        )
    })?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "required framework source directory is a symlink: {} (at {})",
            src_rel,
            source_dir.display()
        );
    }
    if !metadata.is_dir() {
        anyhow::bail!(
            "required framework source path is not a directory: {} (at {})",
            src_rel,
            source_dir.display()
        );
    }
    Ok(source_dir)
}

fn copy_mapped_dir(
    source_dir: &Path,
    src_rel: &str,
    claude_dir: &Path,
    dst_rel: &str,
) -> Result<()> {
    let target_dir = claude_dir.join(dst_rel);
    if let Some(parent) = target_dir.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    // Issue #578: atomically replace the destination so files removed from
    // the source bundle (e.g. legacy Python tools that no longer ship in
    // the post-deyhonification bundle) don't linger forever in
    // `~/.amplihack/.claude/`. Pre-fix `copy_dir_recursive` was purely
    // additive — it never removed destination entries that were absent in
    // the source — which left 300+ stale `.py` files in `tools/amplihack/`
    // on hosts upgraded from amplihack 0.7.x even after a successful rust
    // install. The temp-dir + rename pattern matches `copy_amplifier_bundle`
    // and rolls back on failure so the previous staged tree is preserved.
    //
    // Same-path aliasing (source canonicalises to destination, e.g. when
    // installing from the staged `~/.amplihack` itself) is handled inside
    // `copy_dir_recursive` and would short-circuit the inner copy. To avoid
    // dropping the only copy of the staged tree in that edge case, we
    // canonicalise both ends first and skip the atomic-replace dance when
    // they alias.
    let same_path = target_dir.exists()
        && match (source_dir.canonicalize(), target_dir.canonicalize()) {
            (Ok(s), Ok(t)) => s == t,
            _ => false,
        };

    if same_path {
        // Source IS destination — nothing to copy, nothing to wipe.
        // copy_dir_recursive will print the ↩️ skip line for transparency.
        copy_dir_recursive(source_dir, &target_dir).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_dir.display(),
                target_dir.display()
            )
        })?;
    } else {
        let staging_temp = staging_temp_path(&target_dir);
        if staging_temp.exists() {
            fs::remove_dir_all(&staging_temp).with_context(|| {
                format!(
                    "failed to clean stale staging dir {}",
                    staging_temp.display()
                )
            })?;
        }
        copy_dir_recursive(source_dir, &staging_temp).with_context(|| {
            format!(
                "failed to copy {} to staging dir {}",
                source_dir.display(),
                staging_temp.display()
            )
        })?;

        if target_dir.exists() {
            let backup = backup_path(&target_dir);
            if backup.exists() {
                let _ = fs::remove_dir_all(&backup);
            }
            fs::rename(&target_dir, &backup).with_context(|| {
                format!(
                    "failed to back up existing {} to {}",
                    target_dir.display(),
                    backup.display()
                )
            })?;
            if let Err(err) = fs::rename(&staging_temp, &target_dir) {
                // Roll back to preserve the previously staged tree.
                let _ = fs::rename(&backup, &target_dir);
                let _ = fs::remove_dir_all(&staging_temp);
                return Err(err).with_context(|| {
                    format!(
                        "failed to swap new {} into {} (rolled back to previous tree)",
                        src_rel,
                        target_dir.display()
                    )
                });
            }
            let _ = fs::remove_dir_all(&backup);
        } else {
            fs::rename(&staging_temp, &target_dir).with_context(|| {
                format!(
                    "failed to move new {} into {}",
                    src_rel,
                    target_dir.display()
                )
            })?;
        }
    }

    if dst_rel.starts_with("tools/") {
        let files_updated = set_hook_permissions(&target_dir)?;
        if files_updated > 0 {
            println!("  🔐 Set execute permissions on {files_updated} hook files");
        }
    }
    println!("  ✅ Copied {src_rel} -> {dst_rel}");
    Ok(())
}

/// Staging path for the atomic-rename pattern in `copy_mapped_dir`.
/// Sibling of the target so the rename stays on the same filesystem.
fn staging_temp_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".staging");
    target.with_file_name(name)
}

/// Backup path used during atomic replacement. Sibling of the target so
/// the rename stays on the same filesystem.
fn backup_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".old");
    target.with_file_name(name)
}

/// Stage the `amplifier-bundle/` tree from the source repo into
/// `~/.amplihack/amplifier-bundle/`.
///
/// The dev-orchestrator skill's mandatory execution path
/// (`amplihack recipe run smart-orchestrator`) is unreachable without these
/// recipes (`smart-orchestrator.yaml`, `default-workflow.yaml`,
/// `investigation-workflow.yaml`). See issue #243.
///
/// The bundle is copied to `~/.amplihack/amplifier-bundle/` so the recipe
/// runner's `AMPLIHACK_HOME/amplifier-bundle/recipes` lookup (and the
/// skill's `AMPLIHACK_HOME` auto-detection that walks for an
/// `amplifier-bundle/` folder) both succeed.
///
/// Returns `Ok(true)` if the bundle was staged. Returns an error if the
/// source repo lacks an `amplifier-bundle/` directory, since
/// [`super::settings::missing_framework_paths`] treats the bundle's recipes
/// as required framework assets — a missing source bundle would cause every
/// subsequent launcher boot to attempt
/// (and fail) a re-install in a tight loop.
///
/// The copy is performed via a temp-dir + atomic-rename pattern so a failed
/// mid-flight copy never destroys an existing working bundle.
///
/// The staged copy is compared with its source file for file before the swap,
/// and modes are set by staging itself, not inherited (issue #1955).
///
/// The source `amplifier-bundle/` root must be a real directory; symlinked
/// roots are rejected to prevent a malicious local repo from copying an
/// arbitrary readable directory into the user's staging area.
#[cfg(test)]
pub(super) fn copy_amplifier_bundle(repo_root: &Path, claude_dir: &Path) -> Result<bool> {
    copy_amplifier_bundle_with_manifest(repo_root, claude_dir, None)
}

/// [`copy_amplifier_bundle`], and in a release build the staged copy is also
/// compared with the release manifest embedded in the binary before the swap
/// (issue #1972 R11). `expected` is `None` for `--local` and for a binary with
/// no manifest.
pub(super) fn copy_amplifier_bundle_with_manifest(
    repo_root: &Path,
    claude_dir: &Path,
    expected: Option<&ReleaseManifest>,
) -> Result<bool> {
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
    validate_framework_bundle_compatibility(&source_bundle).with_context(|| {
        format!(
            "source amplifier-bundle at {} is incompatible",
            source_bundle.display()
        )
    })?;

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
    remove_stale_staging(&staging_temp)?;
    if let Err(err) = stage_and_swap_bundle(
        repo_root,
        &source_bundle,
        &staging_temp,
        &target_bundle,
        expected,
    ) {
        // A swap may already have moved staging away, so a missing dir is fine.
        if let Err(cleanup) = fs::remove_dir_all(&staging_temp)
            && cleanup.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "could not remove staging dir {}: {cleanup}",
                staging_temp.display()
            );
        }
        return Err(err);
    }

    validate_staged_framework_bundle(&target_bundle).with_context(|| {
        format!(
            "staged amplifier-bundle at {} is incompatible after atomic replacement",
            target_bundle.display()
        )
    })?;

    println!(
        "  ✅ Staged amplifier-bundle to {}",
        target_bundle.display()
    );
    Ok(true)
}

/// A leftover staging path from an interrupted install, whatever it now is.
fn remove_stale_staging(staging: &Path) -> Result<()> {
    let removed = match fs::symlink_metadata(staging) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => Err(err),
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(staging),
        Ok(_) => fs::remove_file(staging),
    };
    removed.with_context(|| format!("failed to clean stale staging dir {}", staging.display()))
}

/// Create the staging root so only this user can reach it while the copy and
/// its check run (SEC-4). `create` fails if the path reappeared since cleanup.
fn create_private_staging_dir(staging: &Path) -> Result<()> {
    #[cfg(unix)]
    let created = {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(staging)
    };
    #[cfg(not(unix))]
    let created = fs::create_dir(staging);
    created.with_context(|| format!("failed to create staging dir {}", staging.display()))
}

/// Issue #1955 D11: copy, compare with the source, compare with the release
/// manifest (issue #1972), set modes, secure the merge guard, check
/// compatibility, then swap. The caller deletes staging on error.
fn stage_and_swap_bundle(
    repo_root: &Path,
    source_bundle: &Path,
    staging_temp: &Path,
    target_bundle: &Path,
    expected: Option<&ReleaseManifest>,
) -> Result<()> {
    create_private_staging_dir(staging_temp)?;
    copy_dir_recursive(source_bundle, staging_temp).with_context(|| {
        format!(
            "failed to copy amplifier-bundle into staging dir {}",
            staging_temp.display()
        )
    })?;
    #[cfg(test)]
    staging_hook::run(staging_temp);

    let staged = bundle_manifest::of_staged(staging_temp)?;
    let diff = bundle_manifest::compare(&bundle_manifest::of_source(source_bundle)?, &staged);
    if !diff.is_clean() {
        anyhow::bail!("{}", bundle_manifest::render_mismatch(repo_root, &diff));
    }
    // R11: closes the window between the source check and the copy. Uses the
    // staged listing, so a stray file with an excluded name counts as extra.
    if let Some(expected) = expected
        && let Some(diff) = expected.check(&staged)
    {
        let subject = format!("the staged copy at {}", sanitize_path(staging_temp));
        anyhow::bail!(
            "{}\nNothing was swapped in; the installed bundle is unchanged.",
            bundle_manifest::render_release_mismatch(&subject, &expected.version, &diff)
        );
    }

    normalize_bundle_modes(staging_temp)?;
    // After the mode rule, so the guard's exact modes are the ones kept.
    secure_merge_guard(staging_temp)?;
    validate_staged_framework_bundle(staging_temp).with_context(|| {
        format!(
            "copied amplifier-bundle staging dir {} is incompatible",
            staging_temp.display()
        )
    })?;

    if target_bundle.exists() {
        let backup = target_bundle.with_extension("old");
        if backup.exists() {
            fs::remove_dir_all(&backup).ok();
        }
        fs::rename(target_bundle, &backup).with_context(|| {
            format!(
                "failed to back up existing amplifier-bundle at {}",
                target_bundle.display()
            )
        })?;
        if let Err(err) = fs::rename(staging_temp, target_bundle) {
            // Roll the previous bundle back into place so the install isn't bricked.
            let _ = fs::rename(&backup, target_bundle);
            return Err(err).with_context(|| {
                format!(
                    "failed to swap new amplifier-bundle into {}",
                    target_bundle.display()
                )
            });
        }
        let _ = fs::remove_dir_all(&backup);
    } else {
        fs::rename(staging_temp, target_bundle).with_context(|| {
            format!(
                "failed to move new amplifier-bundle into {}",
                target_bundle.display()
            )
        })?;
    }
    Ok(())
}

/// Apply the mode rule across a staged bundle (issue #1955): directories and
/// files with any execute bit get `0755`, other files `0644`.
///
/// A directory is opened up before it is read, so one with no search permission
/// can't hide what is under it. The root stays `0700` until everything else is
/// done (SEC-4). A symlink or special file is an error, because `set_permissions`
/// follows links and could change a file outside the bundle (SEC-12).
#[cfg(unix)]
pub(super) fn normalize_bundle_modes(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let set_mode = |path: &Path, mode: u32| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("failed to set mode {mode:o} on {}", path.display()))
    };
    let root_meta = fs::symlink_metadata(root)
        .with_context(|| format!("failed to inspect {}", root.display()))?;
    if !root_meta.is_dir() {
        anyhow::bail!(
            "the staged bundle root {} is not a directory",
            sanitize_path(root)
        );
    }
    set_mode(root, 0o700)?;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in
            fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))?
        {
            let entry = entry.with_context(|| format!("failed to read {}", dir.display()))?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)
                .with_context(|| format!("failed to inspect {}", path.display()))?;
            let kind = meta.file_type();
            if kind.is_dir() {
                set_mode(&path, 0o755)?;
                pending.push(path);
            } else if kind.is_file() {
                let executable = meta.permissions().mode() & 0o111 != 0;
                set_mode(&path, if executable { 0o755 } else { 0o644 })?;
            } else if kind.is_symlink() {
                anyhow::bail!(
                    "the staged bundle holds a symlink: {}",
                    sanitize_path(&path)
                );
            } else {
                anyhow::bail!(
                    "the staged bundle holds a special file: {}",
                    sanitize_path(&path)
                );
            }
        }
    }
    set_mode(root, 0o755)
}

#[cfg(not(unix))]
pub(super) fn normalize_bundle_modes(_root: &Path) -> Result<()> {
    Ok(())
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

/// Issue #1551: `amplihack recipe run` refuses a gh merge guard that group or
/// others can write, so the staged shim and its directory get exactly 0755
/// whatever mode the checkout and its umask gave them. The `tools/` directory
/// and the gate files it trusts lose their group and other write bits for the
/// same reason. A bundle without the guard is left alone.
fn secure_merge_guard(bundle: &Path) -> Result<()> {
    let tools = bundle.join("tools");
    let dir = tools.join("gh-merge-guard");
    let shim = dir.join("gh");
    if !shim.is_file() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use crate::commands::recipe::run::merge_guard::GATE_FILES;
        use std::os::unix::fs::PermissionsExt;
        let failed = |path: &Path| {
            format!(
                "failed to remove group and other write access from {} (#1551)",
                path.display()
            )
        };
        let bash_env = dir.join(crate::commands::recipe::run::merge_guard::BASH_ENV_FILE);
        if bash_env.is_file() {
            fs::set_permissions(&bash_env, fs::Permissions::from_mode(0o644)).with_context(
                || {
                    format!(
                        "failed to set mode 0644 on the merge guard {} (#1551)",
                        bash_env.display()
                    )
                },
            )?;
        }
        for path in [&shim, &dir] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).with_context(|| {
                format!(
                    "failed to set mode 0755 on the merge guard {} (#1551)",
                    path.display()
                )
            })?;
        }
        let gate_files = GATE_FILES.iter().map(|name| tools.join(name));
        for path in std::iter::once(tools.clone()).chain(gate_files.filter(|p| p.is_file())) {
            let mode = fs::metadata(&path)
                .with_context(|| failed(&path))?
                .permissions()
                .mode();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o7755))
                .with_context(|| failed(&path))?;
        }
    }
    Ok(())
}

/// Test seam: `copy_amplifier_bundle` calls the installed hook with the staging
/// directory after the copy and before the manifest comparison. Issue #1955.
#[cfg(test)]
pub(super) mod staging_hook {
    use std::cell::RefCell;
    use std::path::Path;

    type Hook = Box<dyn FnMut(&Path)>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Run `body` with `hook` installed on this thread.
    pub(in super::super) fn with_staging_hook<R>(
        hook: impl FnMut(&Path) + 'static,
        body: impl FnOnce() -> R,
    ) -> R {
        HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
        let result = body();
        HOOK.with(|slot| *slot.borrow_mut() = None);
        result
    }

    pub(in super::super) fn run(staging: &Path) {
        HOOK.with(|slot| {
            if let Some(hook) = slot.borrow_mut().as_mut() {
                hook(staging);
            }
        });
    }
}
