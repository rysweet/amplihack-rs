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
///
/// Issue #1900: `issue` is the tracking issue the workstream's recipe works
/// against (`issue_number` in its context). `id` is the workstream's own key
/// (work dir, state file, governor identity). An entry needs at least one of
/// them; with no `issue` the workstream uses local tracking. Several entries
/// may share one `issue` as long as each has its own `id`. There is no
/// placeholder value: `"TBD"` or any other non-numeric `issue` is rejected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamConfig {
    #[serde(default)]
    pub issue: serde_json::Value,
    #[serde(default)]
    pub id: serde_json::Value,
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
    /// Extra recipe context for the workstream, such as the caller's
    /// `create_issue` and `remote_host_type` (#1887, #1900). The keys the
    /// orchestrator sets itself (`task_description`, `issue_number`, ...) win.
    #[serde(default)]
    pub context: serde_json::Map<String, serde_json::Value>,
}

/// A positive integer from a JSON number or a decimal string; `None` otherwise.
fn positive_id(value: &serde_json::Value) -> Option<i64> {
    let n = match value {
        serde_json::Value::Number(n) => n.as_i64()?,
        serde_json::Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    (n > 0).then_some(n)
}

/// `true` when `value` means "no value": absent, `null` or a blank string.
fn is_unset(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

impl WorkstreamConfig {
    /// The tracking issue, `None` for local tracking. A set `issue` that is
    /// not a positive number (`"TBD"`, `"abc"`, `-1`) is an error, never a
    /// placeholder (#1900).
    pub fn tracking_issue(&self) -> Result<Option<i64>, String> {
        if is_unset(&self.issue) {
            return Ok(None);
        }
        positive_id(&self.issue).map(Some).ok_or_else(|| {
            format!(
                "Invalid issue number: {} (give a positive issue number, or leave `issue` \
                 out and set `id` for a workstream with local tracking; \"TBD\" is not supported)",
                self.issue
            )
        })
    }

    /// The workstream's key: `id` when set, else the tracking issue.
    pub fn workstream_id(&self) -> Result<i64, String> {
        let tracking = self.tracking_issue()?;
        if !is_unset(&self.id) {
            return positive_id(&self.id).ok_or_else(|| {
                format!(
                    "Invalid workstream id: {} (must be a positive number)",
                    self.id
                )
            });
        }
        tracking.ok_or_else(|| {
            "Workstream has neither `issue` nor `id`; set `issue` to a tracking issue \
             number or `id` to a positive number for local tracking"
                .to_string()
        })
    }

    pub fn description_or_default(&self) -> String {
        self.description
            .clone()
            .unwrap_or_else(|| match self.tracking_issue() {
                Ok(Some(n)) => format!("Issue #{n}"),
                _ => format!("Workstream {}", self.workstream_id().unwrap_or(0)),
            })
    }
}

/// Runtime state for a single workstream.
#[derive(Debug)]
pub struct Workstream {
    /// The workstream's key: the config's `id`, else its `issue` (#1900).
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
    pub workstream_scope: WorkstreamScope,
    pub process_scope: ProcessScope,
    /// The tracking issue passed as `issue_number`; `None` means local
    /// tracking. `issue` above is the workstream's key (#1900).
    pub tracking_issue: Option<i64>,
    /// Extra recipe context from the config entry (#1887, #1900).
    pub extra_context: serde_json::Map<String, serde_json::Value>,
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
            workstream_scope: WorkstreamScope::default(),
            process_scope: ProcessScope::default(),
            tracking_issue: Some(issue),
            extra_context: serde_json::Map::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_id() {
        assert_eq!(sanitize_id("123"), "123");
        assert_eq!(sanitize_id("hello-world"), "hello-world");
        assert_eq!(sanitize_id("path/../../etc"), "path_______etc");
        assert_eq!(sanitize_id("a b c"), "a_b_c");
    }

    #[test]
    fn test_workstream_config_parsing() {
        let json = r#"[
            {
                "issue": 42,
                "branch": "feat/test",
                "description": "Test workstream",
                "task": "Do something",
                "recipe": "default-workflow"
            }
        ]"#;
        let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].tracking_issue(), Ok(Some(42)));
        assert_eq!(configs[0].workstream_id(), Ok(42));
        assert_eq!(configs[0].branch, "feat/test");
        assert_eq!(configs[0].description_or_default(), "Test workstream");
    }

    #[test]
    fn test_workstream_config_string_issue() {
        let json = r#"[{"issue": "99", "branch": "b", "task": "t"}]"#;
        let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
        assert_eq!(configs[0].tracking_issue(), Ok(Some(99)));
    }

    #[test]
    fn test_workstream_config_default_description() {
        let json = r#"[{"issue": 7, "branch": "b", "task": "t"}]"#;
        let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
        assert_eq!(configs[0].description_or_default(), "Issue #7");
    }

    /// #1900: several workstreams may share the caller's issue; each has its
    /// own `id`, which keys its work dir.
    #[test]
    fn shared_issue_with_distinct_ids() {
        let json = r#"[
            {"id": 11, "issue": 368, "branch": "a", "task": "t"},
            {"id": 12, "issue": "368", "branch": "b", "task": "t"}
        ]"#;
        let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
        assert_eq!(configs[0].tracking_issue(), Ok(Some(368)));
        assert_eq!(configs[1].tracking_issue(), Ok(Some(368)));
        assert_eq!(configs[0].workstream_id(), Ok(11));
        assert_eq!(configs[1].workstream_id(), Ok(12));
    }

    /// #1900: no `issue` means local tracking, keyed by `id`.
    #[test]
    fn id_without_issue_is_local_tracking() {
        let json = r#"[{"id": 5, "branch": "b", "task": "t",
                        "context": {"create_issue": "false"}}]"#;
        let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
        assert_eq!(configs[0].tracking_issue(), Ok(None));
        assert_eq!(configs[0].workstream_id(), Ok(5));
        assert_eq!(configs[0].context["create_issue"], "false");
        assert_eq!(configs[0].description_or_default(), "Workstream 5");
    }

    /// #1900: "TBD" is not a placeholder any more; it is refused with a
    /// message that says what to do instead, even when `id` is set.
    #[test]
    fn tbd_issue_is_rejected() {
        for json in [
            r#"[{"issue": "TBD", "branch": "b", "task": "t"}]"#,
            r#"[{"issue": "TBD", "id": 3, "branch": "b", "task": "t"}]"#,
        ] {
            let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
            let err = configs[0].workstream_id().unwrap_err();
            assert!(
                err.contains("Invalid issue number") && err.contains("local tracking"),
                "{err}"
            );
        }
        let none: Vec<WorkstreamConfig> =
            serde_json::from_str(r#"[{"branch": "b", "task": "t"}]"#).unwrap();
        assert!(none[0].workstream_id().unwrap_err().contains("neither"));
    }

    #[test]
    fn test_persisted_state_roundtrip() {
        let state = PersistedState {
            issue: 42,
            branch: "feat/test".to_string(),
            lifecycle_state: "completed".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: PersistedState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.issue, 42);
        assert_eq!(parsed.branch, "feat/test");
        assert_eq!(parsed.lifecycle_state, "completed");
    }

    #[test]
    fn test_derive_cleanup_eligible() {
        assert!(Workstream::derive_cleanup_eligible("completed"));
        assert!(Workstream::derive_cleanup_eligible("failed_terminal"));
        assert!(Workstream::derive_cleanup_eligible("abandoned"));
        assert!(!Workstream::derive_cleanup_eligible("running"));
        assert!(!Workstream::derive_cleanup_eligible("pending"));
        assert!(!Workstream::derive_cleanup_eligible("failed_resumable"));
    }

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_MAX_RUNTIME, 7200);
        assert_eq!(DEFAULT_TIMEOUT_POLICY, "continue-preserve");
    }
}
