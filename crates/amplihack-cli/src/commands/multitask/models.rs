//! Data models for the parallel workstream orchestrator.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Instant;

/// Regex for sanitizing IDs to filesystem-safe strings.
static SAFE_ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-zA-Z0-9_-]").unwrap());

/// Sanitize an identifier for use in filesystem paths.
pub fn sanitize_id(raw: &str) -> String {
    SAFE_ID_RE.replace_all(raw, "_").to_string()
}

/// Default max runtime for workstreams (2 hours).
pub const DEFAULT_MAX_RUNTIME: u64 = 7200;

/// Timeout policy: interrupt the subprocess and preserve state.
pub const INTERRUPT_PRESERVE_TIMEOUT_POLICY: &str = "interrupt-preserve";
/// Timeout policy: mark timed-out but let the subprocess continue.
pub const CONTINUE_PRESERVE_TIMEOUT_POLICY: &str = "continue-preserve";
/// Default timeout policy.
pub const DEFAULT_TIMEOUT_POLICY: &str = INTERRUPT_PRESERVE_TIMEOUT_POLICY;

/// Lifecycle states that can be resumed.
#[allow(dead_code)]
pub const RESUMABLE_STATES: &[&str] = &[
    "failed_resumable",
    "timed_out_resumable",
    "interrupted_resumable",
];

/// Lifecycle states eligible for cleanup.
pub const CLEANUP_ELIGIBLE_STATES: &[&str] = &["completed", "failed_terminal", "abandoned"];

/// JSON config entry for a workstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamConfig {
    pub issue: serde_json::Value,
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
}

impl WorkstreamConfig {
    pub fn issue_id(&self) -> i64 {
        match &self.issue {
            serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
            serde_json::Value::String(s) => s.parse().unwrap_or(0),
            _ => 0,
        }
    }

    pub fn description_or_default(&self) -> String {
        self.description
            .clone()
            .unwrap_or_else(|| format!("Issue #{}", self.issue_id()))
    }
}

/// Runtime state for a single workstream.
#[derive(Debug)]
pub struct Workstream {
    pub issue: i64,
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
}

impl Workstream {
    pub fn new(
        issue: i64,
        branch: String,
        description: String,
        task: String,
        recipe: String,
        base_dir: &Path,
        state_dir: &Path,
    ) -> Self {
        let safe_id = sanitize_id(&issue.to_string());
        Self {
            issue,
            branch,
            description,
            task,
            recipe,
            work_dir: base_dir.join(format!("ws-{issue}")),
            log_file: base_dir.join(format!("log-{safe_id}.txt")),
            state_file: state_dir.join(format!("ws-{safe_id}.json")),
            progress_file: state_dir.join(format!("ws-{safe_id}.progress.json")),
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
        }
    }

    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        if let Some(pid) = self.pid {
            unsafe { libc::kill(pid as i32, 0) == 0 }
        } else {
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
    pub issue: i64,
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
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub resume_context: Option<HashMap<String, serde_json::Value>>,
}
