//! Core data models for the agent system.
//!
//! Matches Python `amplihack/agents/goal_seeking/goal_seeking_agent.py`:
//! - AgentState enum (OODA states)
//! - AgentConfig, AgentInfo, TaskSpec, TaskResult, TaskPriority

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use amplihack_memory::MemoryType;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// AgentState
// ---------------------------------------------------------------------------

/// OODA-loop agent states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Agent is idle, waiting for input.
    Idle,
    /// Observing: gathering input and context.
    Observing,
    /// Orienting: recalling memory and building context.
    Orienting,
    /// Deciding: classifying intent and choosing action.
    Deciding,
    /// Acting: executing the chosen action.
    Acting,
    /// Agent encountered an error.
    Error,
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Observing => write!(f, "observing"),
            Self::Orienting => write!(f, "orienting"),
            Self::Deciding => write!(f, "deciding"),
            Self::Acting => write!(f, "acting"),
            Self::Error => write!(f, "error"),
        }
    }
}

impl AgentState {
    /// Whether this state is terminal (no automatic transition out).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Idle | Self::Error)
    }

    /// The expected next state in the OODA loop.
    pub fn next(&self) -> Option<AgentState> {
        match self {
            Self::Idle => Some(Self::Observing),
            Self::Observing => Some(Self::Orienting),
            Self::Orienting => Some(Self::Deciding),
            Self::Deciding => Some(Self::Acting),
            Self::Acting => Some(Self::Idle),
            Self::Error => None,
        }
    }
}

// ---------------------------------------------------------------------------
// TaskPriority
// ---------------------------------------------------------------------------

/// Priority level for queued tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TaskPriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}


impl fmt::Display for TaskPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Normal => write!(f, "normal"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

// ---------------------------------------------------------------------------
// AgentConfig
// ---------------------------------------------------------------------------

/// Configuration for a goal-seeking agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub agent_name: String,
    pub model: String,
    pub storage_path: PathBuf,
    pub memory_type: MemoryType,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_max_iterations() -> usize {
    10
}

fn default_timeout() -> u64 {
    300
}

impl AgentConfig {
    pub fn new(name: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            agent_name: name.into(),
            model: model.into(),
            storage_path: PathBuf::from(".amplihack/agents"),
            memory_type: MemoryType::Episodic,
            max_iterations: default_max_iterations(),
            timeout_secs: default_timeout(),
        }
    }

    pub fn with_storage_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.storage_path = path.into();
        self
    }

    pub fn with_memory_type(mut self, mt: MemoryType) -> Self {
        self.memory_type = mt;
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

// ---------------------------------------------------------------------------
// AgentInfo
// ---------------------------------------------------------------------------

/// Read-only snapshot of an agent's current status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub agent_name: String,
    pub state: AgentState,
    pub model: String,
    pub iterations: usize,
    pub uptime_secs: f64,
}

// ---------------------------------------------------------------------------
// TaskSpec
// ---------------------------------------------------------------------------

/// Specification for a task to be executed by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    pub description: String,
    #[serde(default)]
    pub priority: TaskPriority,
    #[serde(default = "default_task_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_task_timeout() -> u64 {
    120
}

impl TaskSpec {
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            priority: TaskPriority::Normal,
            timeout_secs: default_task_timeout(),
            tags: Vec::new(),
        }
    }

    pub fn with_priority(mut self, priority: TaskPriority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
}

// ---------------------------------------------------------------------------
// TaskResult
// ---------------------------------------------------------------------------

/// Result of executing a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
    pub duration_secs: f64,
}

impl TaskResult {
    pub fn ok(output: impl Into<String>, duration_secs: f64) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
            duration_secs,
        }
    }

    pub fn fail(error: impl Into<String>, duration_secs: f64) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error.into()),
            duration_secs,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_state_display() {
        assert_eq!(AgentState::Idle.to_string(), "idle");
        assert_eq!(AgentState::Observing.to_string(), "observing");
        assert_eq!(AgentState::Acting.to_string(), "acting");
        assert_eq!(AgentState::Error.to_string(), "error");
    }

    #[test]
    fn agent_state_serde() {
        let json = serde_json::to_string(&AgentState::Deciding).unwrap();
        assert_eq!(json, r#""deciding""#);
        let parsed: AgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, AgentState::Deciding);
    }

    #[test]
    fn agent_state_next_ooda() {
        assert_eq!(AgentState::Idle.next(), Some(AgentState::Observing));
        assert_eq!(AgentState::Observing.next(), Some(AgentState::Orienting));
        assert_eq!(AgentState::Orienting.next(), Some(AgentState::Deciding));
        assert_eq!(AgentState::Deciding.next(), Some(AgentState::Acting));
        assert_eq!(AgentState::Acting.next(), Some(AgentState::Idle));
        assert_eq!(AgentState::Error.next(), None);
    }

    #[test]
    fn agent_state_terminal() {
        assert!(AgentState::Idle.is_terminal());
        assert!(AgentState::Error.is_terminal());
        assert!(!AgentState::Observing.is_terminal());
    }

    #[test]
    fn task_priority_ordering() {
        assert!(TaskPriority::Low < TaskPriority::Normal);
        assert!(TaskPriority::Normal < TaskPriority::High);
        assert!(TaskPriority::High < TaskPriority::Critical);
    }

    #[test]
    fn agent_config_builder() {
        let cfg = AgentConfig::new("test-agent", "gpt-4")
            .with_storage_path("/data")
            .with_memory_type(MemoryType::Semantic)
            .with_timeout(60);
        assert_eq!(cfg.agent_name, "test-agent");
        assert_eq!(cfg.model, "gpt-4");
        assert_eq!(cfg.storage_path, PathBuf::from("/data"));
        assert_eq!(cfg.timeout_secs, 60);
    }

    #[test]
    fn agent_config_defaults() {
        let cfg = AgentConfig::new("a", "m");
        assert_eq!(cfg.max_iterations, 10);
        assert_eq!(cfg.timeout_secs, 300);
        assert_eq!(cfg.memory_type, MemoryType::Episodic);
    }

    #[test]
    fn task_spec_builder() {
        let spec = TaskSpec::new("run tests")
            .with_priority(TaskPriority::High)
            .with_timeout(60);
        assert_eq!(spec.description, "run tests");
        assert_eq!(spec.priority, TaskPriority::High);
        assert_eq!(spec.timeout(), Duration::from_secs(60));
    }

    #[test]
    fn task_result_ok() {
        let r = TaskResult::ok("done", 1.5);
        assert!(r.success);
        assert_eq!(r.output, "done");
        assert!(r.error.is_none());
    }

    #[test]
    fn task_result_fail() {
        let r = TaskResult::fail("boom", 0.1);
        assert!(!r.success);
        assert!(r.output.is_empty());
        assert_eq!(r.error, Some("boom".into()));
    }

    #[test]
    fn task_result_serde_roundtrip() {
        let r = TaskResult::ok("output", 2.0);
        let json = serde_json::to_string(&r).unwrap();
        let parsed: TaskResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.success, true);
        assert_eq!(parsed.output, "output");
    }
}
