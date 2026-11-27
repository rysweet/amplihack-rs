//! Read-only source validation before an installer changes its destination.

use super::bundle_compat::validate_framework_bundle_compatibility;
use super::directories::{find_source_root, required_source_dir};
use super::types::{
    SOURCE_CONDITIONAL_BUNDLE_DIR_MAPPING, SourceLayout, dir_mapping, required_source_files,
};
use anyhow::{Context, Result, bail};
use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Resolve and validate all required release assets without creating destination paths.
/// The layout mappings and legacy home-instruction lookup remain authoritative.
pub(super) fn inspect(repo_root: &Path) -> Result<(PathBuf, SourceLayout)> {
    let (source, layout) = find_source_root(repo_root)?;
    if layout == SourceLayout::Bundle {
        validate_framework_bundle_compatibility(&source)?;
    }
    for (relative, _) in dir_mapping(layout) {
        required_source_dir(&source, relative)?;
    }
    if layout == SourceLayout::Bundle {
        for (relative, _) in SOURCE_CONDITIONAL_BUNDLE_DIR_MAPPING {
            let path = source.join(relative);
            // A dangling symlink is present input too, and must be rejected.
            if fs::symlink_metadata(&path).is_ok() {
                required_source_dir(&source, relative)?;
            }
        }
    }
    let recipes = required_source_dir(&source, "recipes")?;
    for name in ["default-workflow", "investigation-workflow"] {
        let path = recipes.join(format!("{name}.yaml"));
        require_file(&path)?;
        let recipe: Value = serde_yaml::from_slice(&fs::read(&path)?)
            .with_context(|| format!("invalid workflow recipe at {}", path.display()))?;
        let valid = recipe.as_mapping().is_some()
            && recipe.get("name").and_then(Value::as_str) == Some(name)
            && recipe
                .get("steps")
                .and_then(Value::as_sequence)
                .is_some_and(|steps| {
                    !steps.is_empty()
                        && steps.iter().all(|step| {
                            step.as_mapping().is_some()
                                && step
                                    .get("type")
                                    .and_then(Value::as_str)
                                    .is_some_and(|kind| !kind.trim().is_empty())
                        })
                });
        if !valid {
            bail!("incomplete workflow recipe at {}", path.display());
        }
    }
    let skills = required_source_dir(&source, "skills")?;
    for name in [
        "dev-orchestrator",
        "default-workflow",
        "investigation-workflow",
    ] {
        let skill = required_source_dir(&skills, name)?;
        require_file(&skill.join("SKILL.md"))?;
    }
    for relative in required_source_files(layout) {
        require_file(&source.join(relative))?;
    }
    require_file(&home_instructions_source(&source)?)?;
    Ok((source, layout))
}

fn require_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("required release file is missing: {}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
        bail!(
            "required release file must be a nonempty regular file: {}",
            path.display()
        );
    }
    Ok(())
}

/// Share the installer's instruction precedence with preflight.
pub(super) fn home_instructions_source(source: &Path) -> Result<PathBuf> {
    let parent = source.parent().context("source root missing parent")?;
    let candidates = [
        source.join("context/AMPLIHACK_HOME_CLAUDE.md"),
        source.join("CLAUDE.md"),
        parent.join("CLAUDE.md"),
    ];
    candidates
        .into_iter()
        .find(|path| fs::symlink_metadata(path).is_ok())
        .context("required CLAUDE.md home instructions are missing from the source")
}
