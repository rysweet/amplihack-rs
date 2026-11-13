//! Exact task authority comes from matching requirements and reviewed design.
//! Git state and candidate inventories cannot extend it.
use crate::workspace_validation::Workspace;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// A scope can only be constructed by validating both upstream records.
#[derive(Debug, Serialize)]
pub struct TaskScope {
    version: u32,
    checkout: PathBuf,
    project: PathBuf,
    task: String,
    paths: BTreeSet<String>,
}

pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\0', '\n', '\r', '\\'])
        && !Path::new(path).is_absolute()
        && path
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && !path.split('/').any(|s| s.eq_ignore_ascii_case(".git"))
}

fn record(primary: &str, fallback: &str, fields: &[&str]) -> Result<serde_json::Value> {
    let raw = match std::env::var(primary) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => std::env::var(fallback)
            .with_context(|| format!("missing established task authority: {primary}"))?,
        Err(error) => return Err(error).context("unsupported task authority encoding"),
    };
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid established task authority: {primary}"))?;
    let object = value
        .as_object()
        .context("task authority must be an object")?;
    if object.keys().any(|key| !fields.contains(&key.as_str())) {
        bail!("unknown field in task authority: {primary}");
    }
    if value["version"].as_u64() != Some(1) {
        bail!("task authority requires integer version 1: {primary}");
    }
    Ok(value)
}

impl TaskScope {
    pub fn from_recipe(workspace: &Workspace) -> Result<Self> {
        let requirements = record(
            "RECIPE_VAR_final_requirements",
            "RECIPE_VAR_requirements",
            &[
                "version",
                "task",
                "permitted_source_paths",
                "task_summary",
                "explicit_requirements",
                "implicit_requirements",
                "constraints",
                "acceptance_criteria",
                "out_of_scope",
                "ambiguities_resolved",
                "requirements",
                "summary",
                "clarifications",
                "assumptions",
                "success_criteria",
                "consensus_mechanism_used",
                "agents_involved",
                "confidence_level",
                "questions_resolved",
                "estimated_complexity",
                "classification",
                "requires_api_design",
                "requires_database_design",
                "requires_integration_work",
            ],
        )?;
        let design = record(
            "RECIPE_VAR_design_spec",
            "RECIPE_VAR_design_consensus",
            &[
                "version",
                "task",
                "files_to_change",
                "components",
                "new_files",
                "test_files",
                "implementation_order",
                "risks",
                "security_considerations",
                "consensus_achieved",
                "unified_design",
                "trade_offs",
                "dissenting_opinions",
                "rationale",
                "decisions",
                "resolved_conflicts",
                "trade_off_decisions",
                "all_agents_approved",
            ],
        )?;
        let task = requirements["task"]
            .as_str()
            .context("requirements must identify the task")?;
        if task.trim().is_empty() || design["task"].as_str() != Some(task) {
            bail!("requirements and reviewed design task identities disagree");
        }
        let specified = Self::paths(workspace, &requirements["permitted_source_paths"])?;
        let reviewed = Self::paths(workspace, &design["files_to_change"])?;
        let paths = specified
            .intersection(&reviewed)
            .cloned()
            .collect::<BTreeSet<_>>();
        if paths.is_empty() {
            bail!("no exact source paths agreed by requirements and reviewed design");
        }
        Ok(Self {
            version: 1,
            checkout: workspace.root().to_owned(),
            project: workspace.common().to_owned(),
            task: task.to_owned(),
            paths,
        })
    }
    fn paths(workspace: &Workspace, value: &serde_json::Value) -> Result<BTreeSet<String>> {
        value
            .as_array()
            .context("authority must contain an exact filename array")?
            .iter()
            .map(|v| {
                let path = v.as_str().context("task source path must be a string")?;
                if !valid_path(path)
                    || amplihack_types::paths::owned_runtime::prohibited(path)
                    || path == "worktrees"
                    || path.starts_with("worktrees/")
                {
                    bail!("invalid task source filename: {path:?}");
                }
                match std::fs::symlink_metadata(workspace.root().join(path)) {
                    Ok(metadata) if metadata.is_dir() => {
                        bail!("directory-wide task authority is prohibited: {path:?}")
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).context("cannot inspect authorised source filename");
                    }
                }
                Ok(path.to_owned())
            })
            .collect()
    }
    pub(crate) fn check_workspace(&self, workspace: &Workspace) -> Result<()> {
        if self.version != 1
            || self.checkout != workspace.root()
            || self.project != workspace.common()
        {
            bail!("checkpoint scope identity mismatch");
        }
        Ok(())
    }
    pub(crate) fn select(&self, paths: BTreeSet<String>) -> Result<Self> {
        for path in &paths {
            self.require(path)?;
        }
        Ok(Self {
            version: self.version,
            checkout: self.checkout.clone(),
            project: self.project.clone(),
            task: self.task.clone(),
            paths,
        })
    }
    pub(crate) fn read_private(workspace: &Workspace, bytes: &[u8]) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Stored {
            version: u32,
            checkout: PathBuf,
            project: PathBuf,
            task: String,
            paths: serde_json::Value,
        }
        let stored: Stored = serde_json::from_slice(bytes)?;
        let scope = Self {
            version: stored.version,
            checkout: stored.checkout,
            project: stored.project,
            task: stored.task,
            paths: Self::paths(workspace, &stored.paths)?,
        };
        scope.check_workspace(workspace)?;
        if scope.task.trim().is_empty() || scope.paths.is_empty() {
            bail!("empty private task authority");
        }
        Ok(scope)
    }
    pub fn permits(&self, path: &str) -> bool {
        self.paths.contains(path) && !amplihack_types::paths::owned_runtime::prohibited(path)
    }
    pub fn require(&self, path: &str) -> Result<()> {
        if !self.permits(path) {
            return Err(crate::publication_validation::PolicyRefusal(format!(
                "unauthorised source path in task {}: {path}",
                self.task
            ))
            .into());
        }
        Ok(())
    }
}
