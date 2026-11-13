//! Core orchestrator logic for parallel workstream execution.
//!
//! Launcher generation, state persistence helpers, and utility functions
//! live in sibling modules (`launcher`, `state`, `utils`).

use super::models::*;
use super::{
    cleanup, default_branch, dispatch, launcher, ownership, persistence, report, state, utils,
};
use crate::util::format_output_diagnostics;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tracing::warn;

#[cfg(test)]
#[path = "orchestrator_tests.rs"]
mod orchestrator_tests;

pub struct ParallelOrchestrator {
    repo_url: String,
    base_dir: PathBuf,
    state_dir: PathBuf,
    mode: String,
    workstreams: Vec<Workstream>,
    cleaned_up: std::collections::HashSet<WorkstreamKey>,
    freed_bytes: u64,
    default_max_runtime: u64,
    default_timeout_policy: String,
    default_branch: Option<String>,
}

impl ParallelOrchestrator {
    pub fn new(repo_url: &str, mode: &str) -> Self {
        Self {
            repo_url: repo_url.to_string(),
            base_dir: PathBuf::from(utils::default_base_dir()),
            state_dir: PathBuf::from(utils::default_base_dir()).join("state"),
            mode: mode.to_string(),
            workstreams: Vec::new(),
            cleaned_up: std::collections::HashSet::new(),
            freed_bytes: 0,
            default_max_runtime: DEFAULT_MAX_RUNTIME,
            default_timeout_policy: DEFAULT_TIMEOUT_POLICY.to_string(),
            default_branch: None,
        }
    }

    pub fn set_default_max_runtime(&mut self, max_runtime: u64) {
        self.default_max_runtime = max_runtime;
    }

    pub fn set_default_timeout_policy(&mut self, policy: &str) {
        if policy == INTERRUPT_PRESERVE_TIMEOUT_POLICY || policy == CONTINUE_PRESERVE_TIMEOUT_POLICY
        {
            self.default_timeout_policy = policy.to_string();
        } else {
            warn!("Invalid timeout policy {policy:?}; using default {DEFAULT_TIMEOUT_POLICY:?}");
        }
    }

    pub fn running_flag(&self) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(true))
    }

    pub fn setup(&self) -> Result<()> {
        ownership::private_directory(&self.base_dir)?;
        fs::create_dir_all(&self.base_dir)
            .with_context(|| format!("Failed to create base dir: {}", self.base_dir.display()))?;
        ownership::private_directory(&self.state_dir)?;
        utils::check_disk_space(&self.base_dir, 5.0)?;
        Ok(())
    }

    /// Register the `index`-th (one-based) workstream of this run without
    /// touching the filesystem or the network, and return it.
    ///
    /// Issue #2553: an absent issue is not an error. The workstream's identity
    /// is its [`WorkstreamKey`], derived from the configuration, so a
    /// decomposition that knows no issue numbers still produces distinct work
    /// directories, logs, state files and process-map slots.
    pub fn register(
        &mut self,
        index: usize,
        config: &WorkstreamConfig,
        default_recipe: &str,
    ) -> Result<&Workstream> {
        let (idx, _) = self.register_index(index, config, default_recipe)?;
        Ok(&self.workstreams[idx])
    }

    /// [`Self::register`], returning the slot and whether a state file was
    /// read back for it.
    fn register_index(
        &mut self,
        index: usize,
        config: &WorkstreamConfig,
        default_recipe: &str,
    ) -> Result<(usize, bool)> {
        let key = derive_key(config, index)
            .with_context(|| format!("workstream {index} on branch {:?}", config.branch))?;
        // Two entries on one identity would share a work directory, log and
        // state file, and `cleanup_merged` would delete one workstream's work
        // directory on the other's merge.
        if self.workstreams.iter().any(|w| w.key == key) {
            bail!(
                "duplicate workstream identity '{key}': workstream {index} would share \
                 the work directory, log and state file of an earlier workstream in \
                 this run"
            );
        }
        let issue = config.issue()?.map(|n| n.get());

        let mut config = config.clone();
        super::plan::resolve_route(&mut config, default_recipe)?;
        let recipe = config
            .recipe
            .as_deref()
            .unwrap_or(default_recipe)
            .to_string();

        let mut ws = Workstream::new(
            key,
            issue,
            config.branch.clone(),
            config.description_or_default(),
            config.task.clone(),
            recipe,
            &self.base_dir,
            &self.state_dir,
        );

        ws.max_runtime = config.max_runtime.unwrap_or(self.default_max_runtime);
        ws.timeout_policy = config
            .timeout_policy
            .clone()
            .unwrap_or_else(|| self.default_timeout_policy.clone());
        // Issue #1966: copied through unchanged; the recipe validates it.
        ws.is_critical_code = config.is_critical_code.clone();

        ws.classification = config.classification.clone().unwrap();
        ws.dependencies = config.dependencies.clone();
        ws.create_issue = config.create_issue;
        if ws.classification == "Operations" {
            ws.work_dir = std::env::current_dir()?.canonicalize()?;
        }
        let saved = persistence::read_state(&ws.state_file)?;
        if let Some(ref saved) = saved {
            ownership::validate_saved(&ws, saved)?;
            state::apply_saved_state(&mut ws, saved);
        }

        self.workstreams.push(ws);
        Ok((self.workstreams.len() - 1, saved.is_some()))
    }

    pub fn prepare_all(&mut self) -> Result<()> {
        for ws in &self.workstreams {
            ownership::claim(ws)?;
        }
        for idx in 0..self.workstreams.len() {
            self.prepare(idx, self.workstreams[idx].state_file.exists())?;
        }
        Ok(())
    }

    fn prepare(&mut self, idx: usize, had_state: bool) -> Result<()> {
        if self.workstreams[idx].lifecycle_state == "completed" {
            return persistence::persist_state(&self.workstreams[idx]);
        }
        let operations = self.workstreams[idx].classification == "Operations";
        let default_branch = if operations {
            String::new()
        } else {
            self.resolve_default_branch()
        };
        let ws = &mut self.workstreams[idx];
        if !operations {
            let reuse = had_state && ws.work_dir.exists();
            if !reuse {
                // claim() created only an empty, owned artefact directory.
                ownership::prepare_clone(ws)?;
                let mut cmd = amplihack_git::command();
                cmd.args(["clone", "--depth=1"])
                    .arg(format!("--branch={default_branch}"))
                    .arg(&self.repo_url)
                    .arg(&ws.work_dir);
                let output = cmd.output().context("execute git clone")?;
                if !output.status.success() {
                    bail!(
                        "[{}] git clone failed: {}",
                        ws.key,
                        format_output_diagnostics(&output, 400)
                    );
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&ws.artefact_dir, fs::Permissions::from_mode(0o700))?;
                }
                ownership::claim(ws)?;
            }
        }
        launcher::populate_workstream_scope(ws, &self.repo_url, &default_branch);
        if !operations {
            let delegate = launcher::detect_delegate();
            if self.mode == "recipe" {
                launcher::write_recipe_launcher(ws, &delegate)?;
            } else {
                launcher::write_classic_launcher(ws, &delegate)?;
            }
        }

        persistence::persist_state(ws)
    }

    pub fn monitor(&mut self, running: Arc<AtomicBool>) -> Result<()> {
        dispatch::execute(&mut self.workstreams, &self.mode, running)
    }

    pub fn report(&self) -> String {
        report::render(
            &self.mode,
            &self.base_dir,
            &self.workstreams,
            self.cleaned_up.len(),
            self.freed_bytes,
        )
    }

    pub fn cleanup_merged(&self, config_path: &str, dry_run: bool) -> Result<()> {
        cleanup::cleanup_merged(&self.base_dir, &self.state_dir, config_path, dry_run)
    }

    fn resolve_default_branch(&mut self) -> String {
        if let Some(ref branch) = self.default_branch {
            return branch.clone();
        }

        let mut cmd = amplihack_git::command();
        cmd.args(["ls-remote", "--symref", &self.repo_url, "HEAD"]);
        let branch = match cmd.output() {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                default_branch::parse_from_ls_remote(&stdout).unwrap_or_else(|| {
                    eprintln!(
                        "WARNING: git ls-remote did not return a usable HEAD symref for {}; falling back to main. stdout/stderr diagnostic: {}",
                        self.repo_url,
                        format_output_diagnostics(&output, 400)
                    );
                    "main".to_string()
                })
            }
            Ok(output) => {
                eprintln!(
                    "WARNING: git ls-remote failed for {}; falling back to main. stdout/stderr diagnostic: {}",
                    self.repo_url,
                    format_output_diagnostics(&output, 400)
                );
                "main".to_string()
            }
            Err(err) => {
                eprintln!(
                    "WARNING: git ls-remote timed out or failed for {}; falling back to main. diagnostic: {err:#}",
                    self.repo_url
                );
                "main".to_string()
            }
        };

        self.default_branch = Some(branch.clone());
        branch
    }
}
