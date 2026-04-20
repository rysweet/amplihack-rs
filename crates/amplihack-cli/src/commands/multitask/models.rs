//! Data models for multitask orchestration.

use serde::{Deserialize, Serialize};

/// A workstream definition from the config JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamConfig {
    pub issue: Option<u64>,
    pub branch: String,
    pub description: String,
    pub task: String,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<u64>,
}

/// Runtime state for a workstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamState {
    pub id: String,
    pub config: WorkstreamConfig,
    pub status: WorkstreamStatus,
    pub work_dir: Option<String>,
    pub pid: Option<u32>,
    pub started_at: Option<f64>,
    pub completed_at: Option<f64>,
    pub exit_code: Option<i32>,
}

/// Lifecycle states for a workstream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkstreamStatus {
    Pending,
    Running,
    Completed,
    FailedResumable,
    FailedTerminal,
    TimedOutResumable,
    InterruptedResumable,
    Abandoned,
}

impl std::fmt::Display for WorkstreamStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::FailedResumable => write!(f, "failed_resumable"),
            Self::FailedTerminal => write!(f, "failed_terminal"),
            Self::TimedOutResumable => write!(f, "timed_out_resumable"),
            Self::InterruptedResumable => write!(f, "interrupted_resumable"),
            Self::Abandoned => write!(f, "abandoned"),
        }
    }
}

/// Top-level config file format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultitaskConfig {
    pub workstreams: Vec<WorkstreamConfig>,
    #[serde(default = "default_max_runtime")]
    pub max_runtime: u64,
    #[serde(default)]
    pub recipe: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

fn default_max_runtime() -> u64 {
    7200
}
