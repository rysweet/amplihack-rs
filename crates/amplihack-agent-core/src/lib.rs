//! amplihack-agent-core: Agent lifecycle, session management, and OODA loop.
//!
//! Ports the Python `amplihack/agents/goal_seeking/` subsystem:
//! - `GoalSeekingAgent` with OODA loop (observe/orient/decide/act)
//! - Intent detection and classification
//! - Session management
//! - Priority task queue
//! - Agent lifecycle (start/stop/pause/resume)

pub mod agent;
pub mod error;
pub mod intent;
pub mod lifecycle;
pub mod models;
pub mod session;
pub mod task_queue;

// Re-exports for ergonomic access.
pub use agent::{Agent, GoalSeekingAgent};
pub use error::{AgentError, Result};
pub use intent::{Intent, IntentDetector};
pub use lifecycle::{AgentLifecycle, BasicLifecycle, HealthStatus, LifecycleState};
pub use models::{AgentConfig, AgentInfo, AgentState, TaskPriority, TaskResult, TaskSpec};
pub use session::{AgentSession, SessionManager};
pub use task_queue::TaskQueue;
