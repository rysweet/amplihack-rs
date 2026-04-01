//! Code-graph database path resolution and compatibility shim logic.

use super::super::project_artifact_paths;

use anyhow::{Context, Result, bail};
use std::path::Component;
use std::path::{Path, PathBuf};

pub(super) fn default_code_graph_db_path() -> Result<PathBuf> {
    resolve_code_graph_db_path_for_project(
        &std::env::current_dir()
            .context("failed to resolve current directory for default code graph path")?,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProjectCodeGraphPaths {
    pub(super) neutral: PathBuf,
    pub(super) legacy: PathBuf,
    pub(super) resolved: PathBuf,
}

pub(super) fn project_code_graph_paths(project_root: &Path) -> ProjectCodeGraphPaths {
    let neutral = project_root.join(".amplihack").join("graph_db");
    let legacy = project_root.join(".amplihack").join("kuzu_db");
    ProjectCodeGraphPaths {
        resolved: neutral.clone(),
        neutral,
        legacy,
    }
}

pub(super) fn resolve_project_code_graph_paths(
    project_root: &Path,
) -> Result<ProjectCodeGraphPaths> {
    let mut paths = project_code_graph_paths(project_root);
    if !paths.legacy.exists() || paths.neutral.exists() {
        return Ok(paths);
    }

    let canonical_project_root = project_root.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project root while validating legacy graph DB shim: {}",
            project_root.display()
        )
    })?;
    match paths.legacy.canonicalize() {
        Ok(canonical_legacy) if canonical_legacy.starts_with(&canonical_project_root) => {
            paths.resolved = paths.legacy.clone();
            Ok(paths)
        }
        Ok(canonical_legacy) => bail!(
            "legacy graph DB shim escapes project root: {} -> {} (project root: {})",
            paths.legacy.display(),
            canonical_legacy.display(),
            canonical_project_root.display()
        ),
        Err(err) => Err(err).with_context(|| {
            format!(
                "failed to canonicalize legacy graph DB shim: {}",
                paths.legacy.display()
            )
        }),
    }
}

pub fn default_code_graph_db_path_for_project(project_root: &Path) -> Result<PathBuf> {
    Ok(project_code_graph_paths(project_root).neutral)
}

pub fn code_graph_compatibility_notice_for_project(
    project_root: &Path,
    db_path_override: Option<&Path>,
) -> Result<Option<String>> {
    if db_path_override.is_some() {
        return Ok(None);
    }

    let graph_override = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    if graph_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(None);
    }

    let legacy_override = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    if legacy_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(Some(
            "using legacy `AMPLIHACK_KUZU_DB_PATH`; prefer `AMPLIHACK_GRAPH_DB_PATH`.".to_string(),
        ));
    }

    let paths = project_code_graph_paths(project_root);
    if paths.legacy.exists() && !paths.neutral.exists() {
        return Ok(Some(format!(
            "using legacy code-graph store `{}` because `{}` is absent; migrate to `graph_db`.",
            paths.legacy.display(),
            paths.neutral.display()
        )));
    }

    Ok(None)
}

pub(super) fn code_graph_compatibility_notice_for_input(
    input_path: &Path,
    db_path_override: Option<&Path>,
) -> Result<Option<String>> {
    if let Some(project_root) = project_root_for_blarify_input(input_path) {
        return code_graph_compatibility_notice_for_project(project_root, db_path_override);
    }
    code_graph_compatibility_notice_for_project(
        &std::env::current_dir()
            .context("failed to resolve current directory for code graph compatibility notice")?,
        db_path_override,
    )
}

fn validate_graph_db_env_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("graph DB path must be absolute: {}", path.display());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "graph DB path must not contain parent traversal: {}",
            path.display()
        );
    }
    for blocked in ["/proc", "/sys", "/dev"] {
        if path.starts_with(blocked) {
            bail!("blocked unsafe path prefix: {blocked}");
        }
    }
    Ok(path.to_path_buf())
}

pub(super) fn graph_db_env_override(var_name: &str) -> Result<Option<PathBuf>> {
    let Some(path) = std::env::var_os(var_name) else {
        return Ok(None);
    };
    if path.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(path);
    let validated = validate_graph_db_env_path(&path)
        .with_context(|| format!("invalid {var_name} override: {}", path.display()))?;
    Ok(Some(validated))
}

pub fn resolve_code_graph_db_path_for_project(project_root: &Path) -> Result<PathBuf> {
    if let Some(path) = graph_db_env_override("AMPLIHACK_GRAPH_DB_PATH")? {
        return Ok(path);
    }
    if let Some(path) = graph_db_env_override("AMPLIHACK_KUZU_DB_PATH")? {
        return Ok(path);
    }
    Ok(resolve_project_code_graph_paths(project_root)?.resolved)
}

pub(super) fn project_root_for_blarify_input(input_path: &Path) -> Option<&Path> {
    let project_root = input_path.parent()?.parent()?;
    (input_path == project_artifact_paths(project_root).blarify_json).then_some(project_root)
}

pub(super) fn infer_code_graph_db_path_from_input(input_path: &Path) -> Result<PathBuf> {
    if let Some(project_root) = project_root_for_blarify_input(input_path) {
        return resolve_code_graph_db_path_for_project(project_root);
    }
    default_code_graph_db_path()
}
