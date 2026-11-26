//! Source-derived post-install completeness verification.

use super::bundle_compat::validate_staged_framework_bundle;
use super::filesystem::walk_dirs;
use super::types::{SOURCE_CONDITIONAL_BUNDLE_DIR_MAPPING, SourceLayout, dir_mapping};
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
    let source_count = immediate_child_dir_count(&source_skills)?;
    let staged_skills = claude_dir.join("skills");
    let staged_count = immediate_child_dir_count(&staged_skills).unwrap_or(0);
    if staged_count < source_count {
        missing.push(format!(
            "staged skills are incomplete: expected at least {source_count} skill directories, found {staged_count} at {}",
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

fn immediate_child_dir_count(path: &Path) -> Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))? {
        let entry = entry?;
        // A checkout's root skills directory contains links to bundled skills.
        // Count their targets so a missing staged skill cannot pass as zero.
        let metadata = fs::metadata(entry.path())
            .with_context(|| format!("failed to stat {}", entry.path().display()))?;
        if metadata.is_dir() {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn checkout_with_linked_skills_requires_every_staged_skill() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let source = temp.path().join("checkout");
        let staged = temp.path().join("staged");
        for (src, dst) in dir_mapping(SourceLayout::LegacyClaude) {
            fs::create_dir_all(source.join(src)).unwrap();
            fs::create_dir_all(staged.join(dst)).unwrap();
        }
        for name in ["first", "second"] {
            let skill = source.join("amplifier-bundle/skills").join(name);
            fs::create_dir_all(&skill).unwrap();
            fs::write(skill.join("SKILL.md"), "skill definition\n").unwrap();
            std::os::unix::fs::symlink(
                Path::new("../amplifier-bundle/skills").join(name),
                source.join("skills").join(name),
            )
            .unwrap();
        }
        fs::create_dir_all(staged.join("skills/first")).unwrap();
        fs::write(staged.join("skills/first/SKILL.md"), "skill definition\n").unwrap();

        let error = verify_install_completeness(&source, SourceLayout::LegacyClaude, &staged)
            .expect_err("a checkout with a missing staged skill must fail verification");
        assert!(error.to_string().contains("staged skills are incomplete"));

        fs::create_dir_all(staged.join("skills/second")).unwrap();
        fs::write(staged.join("skills/second/SKILL.md"), "skill definition\n").unwrap();
        verify_install_completeness(&source, SourceLayout::LegacyClaude, &staged).unwrap();
    }
}
