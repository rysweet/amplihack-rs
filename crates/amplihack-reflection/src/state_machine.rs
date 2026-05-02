//! State machine for the interactive reflection workflow.
//!
//! Port of `amplifier-bundle/tools/amplihack/reflection/state_machine.py`.
//! On-disk JSON layout matches the Python implementation so an existing
//! deployment's state files remain forward-compatible.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionState {
    Idle,
    Analyzing,
    AwaitingApproval,
    CreatingIssue,
    AwaitingWorkDecision,
    StartingWork,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionStateData {
    pub state: ReflectionState,
    #[serde(default)]
    pub analysis: Option<serde_json::Value>,
    #[serde(default)]
    pub issue_url: Option<String>,
    #[serde(default = "now_ts")]
    pub timestamp: f64,
    #[serde(default)]
    pub session_id: Option<String>,
}

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}

impl ReflectionStateData {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            state: ReflectionState::Idle,
            analysis: None,
            issue_url: None,
            timestamp: now_ts(),
            session_id: Some(session_id.into()),
        }
    }
}

/// File-backed state machine; one state file per session.
pub struct ReflectionStateMachine {
    session_id: String,
    state_file: PathBuf,
}

impl ReflectionStateMachine {
    /// Create a state machine for `session_id` rooted at `runtime_dir`.
    /// `runtime_dir` is created if missing.
    pub fn new(session_id: impl Into<String>, runtime_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(runtime_dir)?;
        let session_id = session_id.into();
        let state_file = runtime_dir.join(format!("reflection_state_{session_id}.json"));
        Ok(Self {
            session_id,
            state_file,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn state_file_path(&self) -> &Path {
        &self.state_file
    }

    /// Read state. If the file is missing or unparseable, return Idle.
    pub fn read_state(&self) -> anyhow::Result<ReflectionStateData> {
        if !self.state_file.exists() {
            return Ok(ReflectionStateData::new(&self.session_id));
        }
        let bytes = match std::fs::read(&self.state_file) {
            Ok(b) => b,
            Err(_) => return Ok(ReflectionStateData::new(&self.session_id)),
        };
        match serde_json::from_slice::<ReflectionStateData>(&bytes) {
            Ok(d) => Ok(d),
            Err(_) => Ok(ReflectionStateData::new(&self.session_id)),
        }
    }

    pub fn write_state(&self, data: &ReflectionStateData) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(data)?;
        std::fs::write(&self.state_file, bytes)?;
        Ok(())
    }

    pub fn reset(&self) -> anyhow::Result<()> {
        if self.state_file.exists() {
            let _ = std::fs::remove_file(&self.state_file);
        }
        Ok(())
    }

    /// Whether the transition `from -> to` is permitted by the workflow.
    pub fn can_transition(&self, from: ReflectionState, to: ReflectionState) -> bool {
        use ReflectionState::*;
        matches!(
            (from, to),
            (Idle, Analyzing)
                | (Analyzing, AwaitingApproval)
                | (Analyzing, Completed)
                | (AwaitingApproval, CreatingIssue)
                | (AwaitingApproval, Completed)
                | (CreatingIssue, AwaitingWorkDecision)
                | (CreatingIssue, Completed)
                | (AwaitingWorkDecision, StartingWork)
                | (AwaitingWorkDecision, Completed)
                | (StartingWork, Completed)
        )
    }

    /// Detect coarse user intent ("approve" / "reject") from a free-form message.
    pub fn detect_user_intent(&self, message: &str) -> Option<&'static str> {
        let m = message.to_ascii_lowercase();
        let m = m.trim();
        const APPROVE: &[&str] = &[
            "yes",
            " y ",
            "create issue",
            "go ahead",
            "approve",
            "ok",
            "sure",
            "do it",
            "proceed",
        ];
        const REJECT: &[&str] = &["no", " n ", "skip", "cancel", "ignore", "don't", "do not"];
        let padded = format!(" {m} ");
        if APPROVE.iter().any(|w| padded.contains(w)) {
            return Some("approve");
        }
        if REJECT.iter().any(|w| padded.contains(w)) {
            return Some("reject");
        }
        None
    }
}
