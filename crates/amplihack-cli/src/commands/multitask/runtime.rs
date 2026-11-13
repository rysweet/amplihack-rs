//! Durable identity-keyed execution records and ownership checks.
use super::{
    completion,
    identity::WorkstreamId,
    models::{ProcessScope, WorkstreamConfig},
    plan, utils,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Runtime {
    #[serde(flatten)]
    pub config: WorkstreamConfig,
    pub revision: u64,
    #[serde(default)]
    pub state_file: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy: Option<super::models::PersistedState>,
    #[serde(deserialize_with = "plan::deserialize_lifecycle")]
    pub lifecycle: String,
    pub work_dir: PathBuf,
    pub artefact_dir: PathBuf,
    pub log_file: PathBuf,
    pub progress_sidecar: PathBuf,
    pub process_scope: ProcessScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default)]
    pub tracking_attempts: Vec<Value>,
    #[serde(default)]
    pub operation_attempts: Vec<Value>,
    #[serde(default)]
    pub resolutions: Vec<Value>,
    #[serde(default)]
    pub blocked_by: Vec<WorkstreamId>,
    #[serde(default)]
    pub last_exit_code: Option<i32>,
    #[serde(default)]
    pub diagnostic: String,
    pub run_id: WorkstreamId,
}

pub fn base_dir() -> PathBuf {
    PathBuf::from(utils::default_base_dir())
}

pub fn state_path(id: WorkstreamId) -> PathBuf {
    base_dir().join("state").join(format!("ws-{id}.json"))
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("missing state parent")?;
    if !parent.exists() {
        plan::private_directory(parent)?;
    }
    ensure!(parent.is_dir(), "JSON parent is not a directory");
    ensure!(!path.is_symlink(), "refusing symlink {}", path.display());
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.as_file().sync_all()?;
    file.persist(path)
        .with_context(|| format!("persist {}", path.display()))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn owned(path: &Path, root: &Path) -> Result<()> {
    ensure!(
        !path.is_symlink(),
        "owned path is a symlink: {}",
        path.display()
    );
    let resolved = if path.exists() {
        path.canonicalize()?
    } else {
        path.parent()
            .context("missing path parent")?
            .canonicalize()?
            .join(path.file_name().context("missing filename")?)
    };
    ensure!(
        resolved.starts_with(root.canonicalize()?) && resolved != root.canonicalize()?,
        "path escaped owned runtime root: {}",
        path.display()
    );
    Ok(())
}

impl Runtime {
    pub fn id(&self) -> WorkstreamId {
        self.config.workstream_id.expect("validated identity")
    }
    pub fn save(&mut self) -> Result<()> {
        self.revision = self
            .revision
            .checked_add(1)
            .context("state revision overflow")?;
        atomic_json(&self.state_file, self)
    }
    pub fn fresh(config: WorkstreamConfig, repository: &Path) -> Result<Self> {
        let id = config.workstream_id.context("missing execution identity")?;
        let artefact_dir = base_dir().join(format!("ws-{id}"));
        ensure!(
            !artefact_dir.is_symlink(),
            "artefact directory is a symlink"
        );
        plan::private_directory(&artefact_dir)?;
        owned(&artefact_dir, &base_dir())?;
        let work_dir = if config.classification != "Operations" {
            artefact_dir.join("repository")
        } else {
            repository.to_owned()
        };
        Ok(Self {
            config,
            revision: 0,
            state_file: state_path(id),
            legacy: None,
            lifecycle: "pending".into(),
            work_dir,
            log_file: artefact_dir.join("output.log"),
            progress_sidecar: artefact_dir.join("progress.json"),
            artefact_dir,
            process_scope: ProcessScope::default(),
            result: None,
            tracking_attempts: vec![],
            operation_attempts: vec![],
            resolutions: vec![],
            blocked_by: vec![],
            last_exit_code: None,
            diagnostic: String::new(),
            run_id: WorkstreamId::new(),
        })
    }
    pub fn load(config: WorkstreamConfig, repository: &Path) -> Result<Self> {
        let id = config.workstream_id.context("missing execution identity")?;
        let Some((path, value)) = super::migration::locate(&config)? else {
            return Self::fresh(config, repository);
        };
        if value.get("workstream_id").is_none() {
            return super::migration::adopt(config, repository, &path, value);
        }
        let mut state: Self = serde_json::from_value(value)
            .context("invalid saved state; refusing fresh execution")?;
        if state.state_file.as_os_str().is_empty() {
            state.state_file = path.clone();
        }
        ensure!(state.state_file == path, "saved state path mismatch");
        ensure!(
            state.config.workstream_id == Some(id) && state.config.repository == config.repository,
            "saved state repository/identity mismatch"
        );
        ensure!(
            state.config.key == config.key
                && state.config.task == config.task
                && state.config.branch == config.branch
                && state.config.classification == config.classification
                && state.config.recipe == config.recipe
                && state.config.depends_on == config.depends_on
                && state.config.required_outputs == config.required_outputs,
            "saved plan changed; reconcile or use the original configuration"
        );
        if !config.issue.is_null() {
            if state.config.issue.is_null() {
                state.config.issue = config.issue.clone();
            }
            ensure!(
                config.issue == state.config.issue,
                "saved tracking conflicts with supplied issue"
            );
        }
        ensure!(
            serde_json::to_value(&config.tracking)?
                == serde_json::to_value(&state.config.tracking)?,
            "saved tracking request changed"
        );
        owned(&state.artefact_dir, &base_dir())?;
        let runtime_root = base_dir();
        for p in [&state.log_file, &state.progress_sidecar] {
            owned(
                p,
                if state.legacy.is_some() {
                    &runtime_root
                } else {
                    &state.artefact_dir
                },
            )?;
        }
        if config.classification != "Operations" {
            owned(
                &state.work_dir,
                if state.legacy.is_some() {
                    &runtime_root
                } else {
                    &state.artefact_dir
                },
            )?;
        } else {
            ensure!(
                state.work_dir.canonicalize()? == repository.canonicalize()?,
                "saved repository directory mismatch"
            );
        }
        if state.lifecycle == "succeeded" {
            completion::validate(
                state
                    .result
                    .as_ref()
                    .context("saved success lacks result")?,
                &state.config,
            )?;
        }
        for attempt in state
            .tracking_attempts
            .iter()
            .chain(&state.operation_attempts)
        {
            ensure!(
                attempt["schema_version"] == 1
                    && attempt["workstream_id"] == serde_json::json!(id)
                    && attempt["repository"] == state.config.repository,
                "saved attempt ownership or schema mismatch"
            );
            let _: WorkstreamId = serde_json::from_value(attempt["attempt_id"].clone())?;
            ensure!(
                attempt["revision"]
                    .as_u64()
                    .is_some_and(|v| v > 0 && v <= state.revision),
                "saved attempt revision is invalid"
            );
            ensure!(
                [
                    "prepared",
                    "succeeded",
                    "failed",
                    "reconciliation_required",
                    "not_applied"
                ]
                .iter()
                .any(|s| attempt["status"] == *s),
                "unknown saved attempt status"
            );
        }
        Ok(state)
    }
}
