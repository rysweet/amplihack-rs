//! amplihack-agent-core: Agent lifecycle, session management, and OODA loop.
//!
//! Ports the Python `amplihack/agents/goal_seeking/` subsystem:
//! - `GoalSeekingAgent` with OODA loop (observe/orient/decide/act)
//! - Intent detection and classification
//! - Session management
//! - Priority task queue
//! - Agent lifecycle (start/stop/pause/resume)

pub mod action_executor;
pub mod agent;
pub mod agentic_loop;
pub mod answer_synth;
pub mod cognitive_adapter;
pub mod error;
pub mod input_events;
pub mod input_source;
pub mod intent;
pub mod lifecycle;
pub mod models;
pub(crate) mod safe_calc;
pub mod session;
pub mod task_queue;

// Re-exports for ergonomic access.
pub use action_executor::{
    action_calculate, action_read_content, RegistryActionExecutor,
};
pub use agent::{Agent, GoalSeekingAgent};
pub use agentic_loop::{
    ActionExecutor, ActionResult, AgenticLoop, LlmClient, LlmMessage, LoopState, MemoryFacade,
    MemoryFact, MemoryRetriever, ReasoningStep, ReasoningTrace, RetrievalPlan,
    SufficiencyEvaluation, DEFAULT_MODEL,
};
pub use error::{AgentError, Result};
pub use intent::{Intent, IntentDetector, COMMAND_WORDS, QUESTION_WORDS};
pub use lifecycle::{AgentLifecycle, BasicLifecycle, HealthStatus, LifecycleState};
pub use models::{AgentConfig, AgentInfo, AgentState, TaskPriority, TaskResult, TaskSpec};
pub use session::{AgentSession, SessionManager};
pub use task_queue::TaskQueue;
pub use cognitive_adapter::{
    BackendKind, CognitiveAdapter, CognitiveAdapterConfig, CognitiveBackend, HiveFact, HiveStore,
    QualityScorer,
};
