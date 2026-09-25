//! Source-derived post-install completeness verification.

use super::bundle_compat::validate_staged_framework_bundle;
use super::filesystem::walk_dirs;
use super::types::{SOURCE_CONDITIONAL_BUNDLE_DIR_MAPPING, SourceLayout, dir_mapping};
use crate::skill_listing_budget::{
    SKILL_LISTING_BUDGET_CHARS, collect_skill_listing, over_budget_report,
};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn verify_install_completeness(
    source_root: &Path,
    layout: SourceLayout,
    claude_dir: &Path,
) -> Result<()> {
    let mut missing = Vec::new();
    let mut mappings = dir_mapping(layout).to_vec();
    if layout == SourceLayout::Bundle {
        for mapping in SOURCE_CONDITIONAL_BUNDLE_DIR_MAPPING {
            if source_root.join(mapping.0).exists() {
                mappings.push(*mapping);
            }
        }
    }

    for (src_rel, dst_rel) in &mappings {
        let source_dir = source_root.join(src_rel);
        require_real_dir(&source_dir, "source", src_rel)?;
        let target_dir = claude_dir.join(dst_rel);
        if !target_dir.is_dir() {
            missing.push(format!(
                "required framework destination directory is missing: {} (expected at {})",
                dst_rel,
                target_dir.display()
            ));
            continue;
        }

        for rel_dir in real_relative_dirs(&source_dir)? {
            let staged_dir = target_dir.join(&rel_dir);
            if !staged_dir.is_dir() {
                missing.push(format!(
                    "staged framework component missing: {}/{} (expected at {})",
                    dst_rel,
                    rel_dir.display(),
                    staged_dir.display()
                ));
            }
        }
    }

    verify_skill_count(source_root, claude_dir, &mut missing)?;
    verify_skill_description_budget(claude_dir, &mut missing)?;
    verify_staged_bundle(source_root, layout, claude_dir, &mut missing)?;

    if !missing.is_empty() {
        bail!(
            "install completeness verification failed for {}:\n  - {}",
            claude_dir.display(),
            missing.join("\n  - ")
        );
    }

    println!("  ✅ Source-derived framework manifest verified");
    Ok(())
}

fn verify_skill_count(
    source_root: &Path,
    claude_dir: &Path,
    missing: &mut Vec<String>,
) -> Result<()> {
    let source_skills = source_root.join("skills");
    if !source_skills.exists() {
        return Ok(());
    }
    let source_count = immediate_real_child_dir_count(&source_skills)?;
    let staged_skills = claude_dir.join("skills");
    let staged_count = immediate_real_child_dir_count(&staged_skills).unwrap_or(0);
    if staged_count < source_count {
        missing.push(format!(
            "staged skills are incomplete: expected at least {source_count} skill directories, found {staged_count} at {}",
            staged_skills.display()
        ));
    }
    Ok(())
}

/// Fail the install when the staged skills contribute more `name` +
/// `description` text than Claude Code will keep resident (issue #1459).
///
/// Past that limit Claude Code **silently** truncates the tail of its skill
/// listing to bare names. A truncated skill still installs and still runs when
/// invoked by name, but can no longer be matched to a task — the same class of
/// defect as a component that failed to stage, which
/// `amplifier-bundle/context/PHILOSOPHY.md` says install must report loudly.
///
/// This measures `<claude_dir>/skills`, the tree amplihack staged, and nothing
/// else. It never reads the user's own skills directory: someone with 200
/// personal skills must not be locked out of installing amplihack.
///
/// An absent staged tree is an empty listing here, which passes. That is safe
/// only because of the call ordering: if the source bundle has no `skills/`
/// there was nothing to stage and an empty listing is the right answer, and if
/// it does, `verify_skill_count` has already run and pushed the shortfall onto
/// `missing`. Keep this call after it.
fn verify_skill_description_budget(claude_dir: &Path, missing: &mut Vec<String>) -> Result<()> {
    let staged_skills = claude_dir.join("skills");
    let entries = collect_skill_listing(&staged_skills).with_context(|| {
        format!(
            "failed to measure the staged skill listing at {}",
            staged_skills.display()
        )
    })?;

    if let Some(report) = over_budget_report(&entries, SKILL_LISTING_BUDGET_CHARS) {
        // The report carries skill labels and character counts only, never
        // description text — see `skill_listing_budget`'s module docs.
        missing.push(format!(
            "staged {report}\n    at {}\n    If you did not author the skills named above, remove that \
             directory and re-run `amplihack install`.",
            staged_skills.display()
        ));
    }
    Ok(())
}

fn verify_staged_bundle(
    source_root: &Path,
    layout: SourceLayout,
    claude_dir: &Path,
    missing: &mut Vec<String>,
) -> Result<()> {
    if layout != SourceLayout::Bundle {
        return Ok(());
    }
    let staged_bundle = claude_dir
        .parent()
        .context("staging .claude dir missing parent")?
        .join("amplifier-bundle");
    if !staged_bundle.is_dir() {
        missing.push(format!(
            "required staged amplifier-bundle is missing (expected at {})",
            staged_bundle.display()
        ));
        return Ok(());
    }

    for rel_dir in real_relative_dirs(source_root)? {
        let staged_dir = staged_bundle.join(&rel_dir);
        if !staged_dir.is_dir() {
            missing.push(format!(
                "staged amplifier-bundle component missing: {} (expected at {})",
                rel_dir.display(),
                staged_dir.display()
            ));
        }
    }
    if let Err(err) = validate_staged_framework_bundle(&staged_bundle) {
        missing.push(format!(
            "staged amplifier-bundle compatibility check failed at {}: {err}",
            staged_bundle.display()
        ));
    }
    Ok(())
}

fn require_real_dir(path: &Path, label: &str, rel: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("required framework {label} directory is missing: {rel}"))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "required framework {label} directory is a symlink: {}",
            path.display()
        );
    }
    if !metadata.is_dir() {
        bail!(
            "required framework {label} path is not a directory: {}",
            path.display()
        );
    }
    Ok(())
}

fn real_relative_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for path in walk_dirs(root)? {
        if path == root {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .with_context(|| format!("failed to relativize {}", path.display()))?;
        dirs.push(rel.to_path_buf());
    }
    dirs.sort();
    Ok(dirs)
}

fn immediate_real_child_dir_count(path: &Path) -> Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))? {
        let entry = entry?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat {}", entry.path().display()))?;
        if file_type.is_dir() {
            count += 1;
        }
    }
    Ok(count)
}
