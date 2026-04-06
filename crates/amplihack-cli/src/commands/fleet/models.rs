//! Fleet API model DTOs — fields populated from API responses and TUI rendering.
#![allow(dead_code)]

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentStatus {
    Unknown,
    Thinking,
    Running,
    Idle,
    Shell,
    NoSession,
    Unreachable,
    Completed,
    Stuck,
    Error,
    WaitingInput,
}

impl AgentStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Unknown => "unknown",
            AgentStatus::Thinking => "thinking",
            AgentStatus::Running => "running",
            AgentStatus::Idle => "idle",
            AgentStatus::Shell => "shell",
            AgentStatus::NoSession => "no_session",
            AgentStatus::Unreachable => "unreachable",
            AgentStatus::Completed => "completed",
            AgentStatus::Stuck => "stuck",
            AgentStatus::Error => "error",
            AgentStatus::WaitingInput => "waiting_input",
        }
    }

    pub(super) fn summary_icon(self) -> char {
        match self {
            AgentStatus::Thinking => '*',
            AgentStatus::Running => '>',
            AgentStatus::Completed => '=',
            AgentStatus::Stuck => '!',
            AgentStatus::Error => 'X',
            AgentStatus::Idle => '~',
            AgentStatus::Shell => '$',
            AgentStatus::NoSession => '0',
            AgentStatus::Unreachable => 'U',
            AgentStatus::WaitingInput => '?',
            AgentStatus::Unknown => '.',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TmuxSessionInfo {
    pub(super) session_name: String,
    pub(super) vm_name: String,
    pub(super) windows: u32,
    pub(super) attached: bool,
    pub(super) agent_status: AgentStatus,
    pub(super) last_output: String,
    pub(super) working_directory: String,
    pub(super) repo_url: String,
    pub(super) git_branch: String,
    pub(super) pr_url: String,
    pub(super) task_summary: String,
}

#[derive(Debug, Clone)]
pub(super) struct ObservationResult {
    pub(super) session_name: String,
    pub(super) status: AgentStatus,
    pub(super) last_output_lines: Vec<String>,
    pub(super) confidence: f64,
    pub(super) matched_pattern: String,
}

#[derive(Debug, Clone)]
pub(super) struct AuthResult {
    pub(super) service: String,
    pub(super) vm_name: String,
    pub(super) success: bool,
    pub(super) files_copied: Vec<String>,
    pub(super) error: Option<String>,
    pub(super) duration_seconds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionType {
    StartAgent,
    StopAgent,
    ReassignTask,
    MarkComplete,
    MarkFailed,
    Report,
    PropagateAuth,
}

impl ActionType {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            ActionType::StartAgent => "start_agent",
            ActionType::StopAgent => "stop_agent",
            ActionType::ReassignTask => "reassign_task",
            ActionType::MarkComplete => "mark_complete",
            ActionType::MarkFailed => "mark_failed",
            ActionType::Report => "report",
            ActionType::PropagateAuth => "propagate_auth",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct DirectorAction {
    pub(super) action_type: ActionType,
    pub(super) task: Option<FleetTask>,
    pub(super) vm_name: Option<String>,
    pub(super) session_name: Option<String>,
    pub(super) reason: String,
    pub(super) timestamp: String,
}

impl DirectorAction {
    pub(super) fn new(
        action_type: ActionType,
        task: Option<FleetTask>,
        vm_name: Option<String>,
        session_name: Option<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            action_type,
            task,
            vm_name,
            session_name,
            reason: reason.into(),
            timestamp: now_isoformat(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct DirectorLog {
    pub(super) actions: Vec<Value>,
    pub(super) persist_path: Option<PathBuf>,
}

impl DirectorLog {
    pub(super) fn record(&mut self, action: &DirectorAction, outcome: &str) -> Result<()> {
        self.actions.push(serde_json::json!({
            "timestamp": action.timestamp,
            "action": action.action_type.as_str(),
            "vm": action.vm_name,
            "session": action.session_name,
            "task_id": action.task.as_ref().map(|task| task.id.clone()),
            "reason": action.reason,
            "outcome": outcome,
        }));
        self.save()
    }

    pub(super) fn save(&self) -> Result<()> {
        let Some(path) = &self.persist_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(&self.actions)
            .context("failed to serialize admiral action log")?;
        let mut temp =
            tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
                .with_context(|| format!("failed to create temp file for {}", path.display()))?;
        temp.write_all(&bytes)
            .with_context(|| format!("failed to write {}", path.display()))?;
        // SEC-PERM: tempfile guarantees 0o600 on Unix (O_CREAT with mode 0600, unaffected by umask > 0o177)
        temp.persist(path)
            .map_err(|err| err.error)
            .with_context(|| format!("failed to persist {}", path.display()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct AdmiralStats {
    pub(super) actions: usize,
    pub(super) successes: usize,
    pub(super) failures: usize,
}

#[derive(Debug, Clone)]
pub(super) struct VmInfo {
    pub(super) name: String,
    pub(super) session_name: String,
    pub(super) os: String,
    pub(super) status: String,
    pub(super) ip: String,
    pub(super) region: String,
    pub(super) tmux_sessions: Vec<TmuxSessionInfo>,
}

impl VmInfo {
    pub(super) fn is_running(&self) -> bool {
        self.status.to_ascii_lowercase().contains("run")
    }

    pub(super) fn active_agents(&self) -> usize {
        self.tmux_sessions
            .iter()
            .filter(|session| {
                matches!(
                    session.agent_status,
                    AgentStatus::Thinking | AgentStatus::Running | AgentStatus::WaitingInput
                )
            })
            .count()
    }
}
