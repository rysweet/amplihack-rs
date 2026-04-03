//! Agent lifecycle management.
//!
//! Defines the `AgentLifecycle` trait for starting, stopping, pausing,
//! and resuming agents, plus health checks.

use crate::error::Result;

// ---------------------------------------------------------------------------
// LifecycleState
// ---------------------------------------------------------------------------

/// States for agent lifecycle (orthogonal to OODA AgentState).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    /// Agent has not been started.
    Stopped,
    /// Agent is running and accepting work.
    Running,
    /// Agent is temporarily paused.
    Paused,
    /// Agent has failed and requires restart.
    Failed,
}

impl std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "stopped"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

// ---------------------------------------------------------------------------
// HealthStatus
// ---------------------------------------------------------------------------

/// Result of a health check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthStatus {
    pub healthy: bool,
    pub lifecycle_state: LifecycleState,
    pub message: String,
    pub uptime_secs: f64,
}

impl HealthStatus {
    pub fn ok(state: LifecycleState, uptime_secs: f64) -> Self {
        Self {
            healthy: true,
            lifecycle_state: state,
            message: "healthy".into(),
            uptime_secs,
        }
    }

    pub fn unhealthy(state: LifecycleState, message: impl Into<String>) -> Self {
        Self {
            healthy: false,
            lifecycle_state: state,
            message: message.into(),
            uptime_secs: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// AgentLifecycle trait
// ---------------------------------------------------------------------------

/// Lifecycle management for agents.
pub trait AgentLifecycle {
    /// Start the agent. Must be in Stopped state.
    fn start(&mut self) -> Result<()>;

    /// Stop the agent. Must be in Running or Paused state.
    fn stop(&mut self) -> Result<()>;

    /// Pause the agent. Must be in Running state.
    fn pause(&mut self) -> Result<()>;

    /// Resume the agent. Must be in Paused state.
    fn resume(&mut self) -> Result<()>;

    /// Check agent health.
    fn health_check(&self) -> HealthStatus;

    /// Return the current lifecycle state.
    fn lifecycle_state(&self) -> LifecycleState;
}

// ---------------------------------------------------------------------------
// BasicLifecycle — stub implementation
// ---------------------------------------------------------------------------

/// Minimal lifecycle implementation for testing.
///
/// All transition bodies are `todo!()` stubs — tests come first.
#[allow(dead_code)] // Fields used once todo!() stubs are implemented
pub struct BasicLifecycle {
    state: LifecycleState,
    started_at: Option<f64>,
}

impl BasicLifecycle {
    pub fn new() -> Self {
        Self {
            state: LifecycleState::Stopped,
            started_at: None,
        }
    }
}

impl Default for BasicLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentLifecycle for BasicLifecycle {
    fn start(&mut self) -> Result<()> {
        todo!("start: transition Stopped → Running")
    }

    fn stop(&mut self) -> Result<()> {
        todo!("stop: transition Running/Paused → Stopped")
    }

    fn pause(&mut self) -> Result<()> {
        todo!("pause: transition Running → Paused")
    }

    fn resume(&mut self) -> Result<()> {
        todo!("resume: transition Paused → Running")
    }

    fn health_check(&self) -> HealthStatus {
        todo!("health_check: return current health status")
    }

    fn lifecycle_state(&self) -> LifecycleState {
        self.state
    }
}
