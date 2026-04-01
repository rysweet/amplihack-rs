use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskPriority {
    Critical,
    High,
    Medium,
    Low,
}

impl TaskPriority {
    pub(super) fn as_name(self) -> &'static str {
        match self {
            TaskPriority::Critical => "CRITICAL",
            TaskPriority::High => "HIGH",
            TaskPriority::Medium => "MEDIUM",
            TaskPriority::Low => "LOW",
        }
    }

    pub(super) fn short_label(self) -> char {
        match self {
            TaskPriority::Critical => 'C',
            TaskPriority::High => 'H',
            TaskPriority::Medium => 'M',
            TaskPriority::Low => 'L',
        }
    }

    pub(super) fn from_name(value: &str) -> Self {
        match value {
            "CRITICAL" => TaskPriority::Critical,
            "HIGH" => TaskPriority::High,
            "LOW" => TaskPriority::Low,
            _ => TaskPriority::Medium,
        }
    }

    pub(super) fn rank(self) -> u8 {
        match self {
            TaskPriority::Critical => 0,
            TaskPriority::High => 1,
            TaskPriority::Medium => 2,
            TaskPriority::Low => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskStatus {
    Queued,
    Assigned,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Assigned => "assigned",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    pub(super) fn heading(self) -> &'static str {
        match self {
            TaskStatus::Queued => "QUEUED",
            TaskStatus::Assigned => "ASSIGNED",
            TaskStatus::Running => "RUNNING",
            TaskStatus::Completed => "COMPLETED",
            TaskStatus::Failed => "FAILED",
            TaskStatus::Cancelled => "CANCELLED",
        }
    }

    pub(super) fn from_value(value: &str) -> Self {
        match value {
            "assigned" => TaskStatus::Assigned,
            "running" => TaskStatus::Running,
            "completed" => TaskStatus::Completed,
            "failed" => TaskStatus::Failed,
            "cancelled" => TaskStatus::Cancelled,
            _ => TaskStatus::Queued,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FleetTask {
    pub(super) id: String,
    pub(super) prompt: String,
    pub(super) repo_url: String,
    pub(super) branch: String,
    pub(super) priority: TaskPriority,
    pub(super) status: TaskStatus,
    pub(super) agent_command: String,
    pub(super) agent_mode: String,
    pub(super) max_turns: u32,
    pub(super) protected: bool,
    pub(super) assigned_vm: Option<String>,
    pub(super) assigned_session: Option<String>,
    pub(super) assigned_at: Option<String>,
    pub(super) created_at: String,
    pub(super) started_at: Option<String>,
    pub(super) completed_at: Option<String>,
    pub(super) result: Option<String>,
    pub(super) pr_url: Option<String>,
    pub(super) error: Option<String>,
}

impl FleetTask {
    pub(super) fn new(
        prompt: &str,
        repo_url: &str,
        priority: TaskPriority,
        agent_command: &str,
        agent_mode: &str,
        max_turns: u32,
    ) -> Self {
        Self {
            id: generate_task_id(prompt),
            prompt: prompt.to_string(),
            repo_url: repo_url.to_string(),
            branch: String::new(),
            priority,
            status: TaskStatus::Queued,
            agent_command: agent_command.to_string(),
            agent_mode: agent_mode.to_string(),
            max_turns,
            protected: false,
            assigned_vm: None,
            assigned_session: None,
            assigned_at: None,
            created_at: now_isoformat(),
            started_at: None,
            completed_at: None,
            result: None,
            pr_url: None,
            error: None,
        }
    }

    pub(super) fn to_json_value(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "prompt": self.prompt,
            "repo_url": self.repo_url,
            "branch": self.branch,
            "priority": self.priority.as_name(),
            "status": self.status.as_str(),
            "agent_command": self.agent_command,
            "agent_mode": self.agent_mode,
            "max_turns": self.max_turns,
            "protected": self.protected,
            "assigned_vm": self.assigned_vm,
            "assigned_session": self.assigned_session,
            "assigned_at": self.assigned_at,
            "created_at": self.created_at,
            "started_at": self.started_at,
            "completed_at": self.completed_at,
            "result": self.result,
            "pr_url": self.pr_url,
            "error": self.error,
        })
    }

    pub(super) fn from_json_value(value: &Value) -> Option<Self> {
        Some(Self {
            id: value.get("id")?.as_str()?.to_string(),
            prompt: value.get("prompt")?.as_str()?.to_string(),
            repo_url: value
                .get("repo_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            branch: value
                .get("branch")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            priority: TaskPriority::from_name(
                value
                    .get("priority")
                    .and_then(Value::as_str)
                    .unwrap_or("MEDIUM"),
            ),
            status: TaskStatus::from_value(
                value
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("queued"),
            ),
            agent_command: value
                .get("agent_command")
                .and_then(Value::as_str)
                .unwrap_or("claude")
                .to_string(),
            agent_mode: value
                .get("agent_mode")
                .and_then(Value::as_str)
                .unwrap_or("auto")
                .to_string(),
            max_turns: value
                .get("max_turns")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_MAX_TURNS as u64) as u32,
            protected: value
                .get("protected")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            assigned_vm: value
                .get("assigned_vm")
                .and_then(Value::as_str)
                .map(str::to_string),
            assigned_session: value
                .get("assigned_session")
                .and_then(Value::as_str)
                .map(str::to_string),
            assigned_at: value
                .get("assigned_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            created_at: value
                .get("created_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            started_at: value
                .get("started_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            completed_at: value
                .get("completed_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            result: value
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_string),
            pr_url: value
                .get("pr_url")
                .and_then(Value::as_str)
                .map(str::to_string),
            error: value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    pub(super) fn assign(&mut self, vm_name: &str, session_name: &str) {
        self.assigned_vm = Some(vm_name.to_string());
        self.assigned_session = Some(session_name.to_string());
        self.assigned_at = Some(now_isoformat());
        self.status = TaskStatus::Assigned;
    }

    pub(super) fn start(&mut self) {
        self.started_at = Some(now_isoformat());
        self.status = TaskStatus::Running;
    }

    pub(super) fn complete(&mut self, result: &str, pr_url: Option<String>) {
        self.completed_at = Some(now_isoformat());
        self.status = TaskStatus::Completed;
        self.result = Some(result.to_string());
        self.pr_url = pr_url;
    }

    pub(super) fn fail(&mut self, error: &str) {
        self.completed_at = Some(now_isoformat());
        self.status = TaskStatus::Failed;
        self.error = Some(error.to_string());
    }

    pub(super) fn requeue(&mut self) {
        self.status = TaskStatus::Queued;
        self.assigned_vm = None;
        self.assigned_session = None;
        self.assigned_at = None;
    }
}

