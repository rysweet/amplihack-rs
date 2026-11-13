//! Data models for the parallel workstream orchestrator.

pub use super::identity::{WorkstreamKey, derive_key, work_dir_for};
use super::issue::{self, IssueNumber};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Default max runtime for workstreams (2 hours).
pub const DEFAULT_MAX_RUNTIME: u64 = 7200;

/// Timeout policy: interrupt the subprocess and preserve state.
pub const INTERRUPT_PRESERVE_TIMEOUT_POLICY: &str = "interrupt-preserve";
/// Timeout policy: mark timed-out but let the subprocess continue.
pub const CONTINUE_PRESERVE_TIMEOUT_POLICY: &str = "continue-preserve";
/// Default timeout policy. Issue #867: a non-killing default — a run that
/// passes `max_runtime` is marked resumable but is NOT interrupted while it is
/// still producing output. Interrupting is opt-in via `interrupt-preserve`,
/// and even then only a genuinely idle child is killed.
pub const DEFAULT_TIMEOUT_POLICY: &str = CONTINUE_PRESERVE_TIMEOUT_POLICY;

/// Lifecycle states eligible for cleanup.
pub const CLEANUP_ELIGIBLE_STATES: &[&str] = &["completed", "failed_terminal", "abandoned"];

/// Logical workflow ownership scope persisted with every workstream.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WorkstreamScope {
    #[serde(default)]
    pub repository: String,
    #[serde(default)]
    pub repo_root: String,
    #[serde(default)]
    pub workdir: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub base_ref: String,
    #[serde(default)]
    pub issue_id: String,
    #[serde(default)]
    pub work_item_id: String,
    #[serde(default)]
    pub recipe: String,
    #[serde(default)]
    pub recipe_run_id: String,
    #[serde(default)]
    pub tree_id: String,
    #[serde(default)]
    pub workstream_id: String,
    #[serde(default)]
    pub expected_title_prefix: String,
    #[serde(default)]
    pub started_at: String,
}

/// Concrete OS process ownership scope persisted with every launch.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ProcessScope {
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub repository: String,
    #[serde(default)]
    pub repo_root: String,
    #[serde(default)]
    pub workdir: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub issue_id: String,
    #[serde(default)]
    pub work_item_id: String,
    #[serde(default)]
    pub recipe_run_id: String,
    #[serde(default)]
    pub tree_id: String,
    #[serde(default)]
    pub workstream_id: String,
    #[serde(default)]
    pub process_started_at: String,
    #[serde(default)]
    pub recorded_at: String,
}

/// JSON config entry for a workstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamConfig {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub classification: Option<String>,
    #[serde(default, alias = "depends_on")]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub create_issue: bool,
    /// The tracked issue, which may be absent. Kept as a raw JSON value at
    /// the wire boundary so that pre-#2553 configuration files still
    /// deserialise; it is only ever read through [`WorkstreamConfig::issue`].
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub issue: serde_json::Value,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub description: Option<String>,
    pub task: String,
    #[serde(default)]
    pub recipe: Option<String>,
    #[serde(default)]
    pub max_runtime: Option<u64>,
    #[serde(default)]
    pub timeout_policy: Option<String>,
    /// Issue #1966: copied unchanged into `context.json` for the consensus
    /// workflow, which validates it. Must be a JSON string when present.
    #[serde(default)]
    pub is_critical_code: Option<String>,
}

impl WorkstreamConfig {
    /// The tracked issue, or `None` when the decomposition did not know one.
    /// Only a value that is neither a well-formed issue nor a recognised
    /// placeholder is an error; see [`super::issue::parse_optional`].
    pub fn issue(&self) -> Result<Option<IssueNumber>> {
        issue::parse_optional(&self.issue)
            .with_context(|| format!("workstream on branch {:?}", self.branch))
    }

    pub fn description_or_default(&self) -> String {
        if let Some(d) = self
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return d.to_string();
        }
        match self.issue().ok().flatten() {
            Some(n) => format!("Issue #{}", n.get()),
            None => self.branch.clone(),
        }
    }
}

/// Runtime state for a single workstream.
#[derive(Debug)]
pub struct Workstream {
    pub classification: String,
    pub dependencies: Vec<String>,
    pub create_issue: bool,
    pub tracking_reference: String,
    pub result: serde_json::Value,
    pub artefact_dir: PathBuf,
    /// Identity: always present, unique within a run, safe in a path.
    pub key: WorkstreamKey,
    /// Tracking: the issue this workstream reports against, if any.
    pub issue: Option<u64>,
    pub branch: String,
    pub description: String,
    pub task: String,
    pub recipe: String,
    pub work_dir: PathBuf,
    pub log_file: PathBuf,
    pub state_file: PathBuf,
    pub progress_file: PathBuf,
    pub pid: Option<u32>,
    pub start_time: Option<Instant>,
    pub end_time: Option<Instant>,
    pub exit_code: Option<i32>,
    pub lifecycle_state: String,
    pub cleanup_eligible: bool,
    pub worktree_path: String,
    pub checkpoint_id: String,
    pub last_step: String,
    pub attempt: u32,
    pub timeout_policy: String,
    pub max_runtime: u64,
    pub resume_checkpoint: String,
    pub workstream_scope: WorkstreamScope,
    pub process_scope: ProcessScope,
    /// Issue #1966: caller-supplied `is_critical_code`, never normalised.
    pub is_critical_code: Option<String>,
}

impl Workstream {
    pub fn new(
        key: WorkstreamKey,
        issue: Option<u64>,
        branch: String,
        description: String,
        task: String,
        recipe: String,
        base_dir: &Path,
        state_dir: &Path,
    ) -> Self {
        let work_dir = work_dir_for(base_dir, &key);
        let log_file = base_dir.join(format!("log-{key}.txt"));
        let state_file = state_dir.join(format!("ws-{key}.json"));
        let progress_file = state_dir.join(format!("ws-{key}.progress.json"));
        Self {
            classification: if recipe == "operations-direct" {
                "Operations"
            } else {
                "Development"
            }
            .into(),
            dependencies: Vec::new(),
            create_issue: false,
            tracking_reference: String::new(),
            result: serde_json::Value::Null,
            artefact_dir: work_dir.clone(),
            key,
            issue,
            branch,
            description,
            task,
            recipe,
            work_dir,
            log_file,
            state_file,
            progress_file,
            pid: None,
            start_time: None,
            end_time: None,
            exit_code: None,
            lifecycle_state: "pending".to_string(),
            cleanup_eligible: false,
            worktree_path: String::new(),
            checkpoint_id: String::new(),
            last_step: String::new(),
            attempt: 0,
            timeout_policy: DEFAULT_TIMEOUT_POLICY.to_string(),
            max_runtime: DEFAULT_MAX_RUNTIME,
            resume_checkpoint: String::new(),
            workstream_scope: WorkstreamScope::default(),
            process_scope: ProcessScope::default(),
            is_critical_code: None,
        }
    }

    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        let Some(pid) = self.pid else {
            return false;
        };
        #[cfg(unix)]
        {
            unsafe { libc::kill(pid as i32, 0) == 0 }
        }
        #[cfg(not(unix))]
        {
            // Windows fallback: assume not running unless we have a tighter
            // signal. The orchestrator does not currently rely on this on
            // Windows; if it ever does, port to OpenProcess + GetExitCodeProcess.
            let _ = pid;
            false
        }
    }

    pub fn runtime_seconds(&self) -> Option<f64> {
        self.start_time.map(|start| {
            let end = self.end_time.unwrap_or_else(Instant::now);
            end.duration_since(start).as_secs_f64()
        })
    }

    pub fn derive_cleanup_eligible(lifecycle_state: &str) -> bool {
        CLEANUP_ELIGIBLE_STATES.contains(&lifecycle_state)
    }
}

/// Persisted state for a workstream (JSON on disk).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistedState {
    #[serde(default)]
    pub classification: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub create_issue: bool,
    #[serde(default)]
    pub tracking_reference: String,
    #[serde(default)]
    pub result: serde_json::Value,
    /// The workstream identity this file belongs to. Recorded for diagnostics
    /// only: a key read back from disk never constructs a path, so a state
    /// directory an attacker can write to cannot point a run at another run's
    /// work.
    #[serde(default)]
    pub key: String,
    /// Absent when the workstream tracks no issue. A pre-#2553 file carrying
    /// `0` reads back as absence; see [`super::issue::deserialize_persisted`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::issue::deserialize_persisted"
    )]
    pub issue: Option<u64>,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub recipe: String,
    #[serde(default)]
    pub lifecycle_state: String,
    #[serde(default)]
    pub cleanup_eligible: bool,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default)]
    pub last_pid: Option<u32>,
    #[serde(default)]
    pub last_exit_code: Option<i32>,
    #[serde(default)]
    pub current_step: String,
    #[serde(default)]
    pub checkpoint_id: String,
    #[serde(default)]
    pub work_dir: String,
    #[serde(default)]
    pub worktree_path: String,
    #[serde(default)]
    pub log_file: String,
    #[serde(default)]
    pub progress_sidecar: String,
    #[serde(default)]
    pub max_runtime: Option<u64>,
    #[serde(default)]
    pub timeout_policy: Option<String>,
    #[serde(default)]
    pub workstream_scope: WorkstreamScope,
    #[serde(default)]
    pub process_scope: ProcessScope,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub resume_context: Option<HashMap<String, serde_json::Value>>,
}
