//! Multi-agent sub-agent architecture for goal-seeking agents.
//!
//! Ports the Python `amplihack/agents/goal_seeking/sub_agents/` package.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────┐
//! │   Coordinator     │  classify(question, intent) → TaskRoute
//! └────────┬─────────┘
//!          │ routes to
//!          ▼
//! ┌──────────────────┐     ┌──────────────────┐
//! │   MemoryAgent     │     │   AgentSpawner    │
//! │ (retrieval strat) │     │ (dynamic spawn)   │
//! └──────────────────┘     └──────────────────┘
//!          │                        │
//!          ▼                        ▼
//!   Vec<MemoryFact>          SpawnedAgent results
//! ```
//!
//! # Modules
//!
//! | File              | Responsibility                                    |
//! |-------------------|---------------------------------------------------|
//! | `types.rs`        | Shared enums, structs, traits, and constants      |
//! | `coordinator.rs`  | Task classification and routing                   |
//! | `memory_agent.rs` | Retrieval strategy selection per question type     |
//! | `agent_spawner.rs`| Dynamic specialist agent spawning                 |
//! | `multi_agent.rs`  | Multi-agent orchestration pipeline                |
//! | `tool_injector.rs`| SDK-specific tool capability injection             |

pub mod agent_spawner;
pub mod coordinator;
pub mod memory_agent;
pub mod multi_agent;
pub mod tool_injector;
pub mod types;

// Re-exports for ergonomic access.
pub use agent_spawner::AgentSpawner;
pub use coordinator::CoordinatorAgent;
pub use memory_agent::MemoryAgent;
pub use multi_agent::{AnswerContext, MultiAgentConfig, MultiAgentOrchestrator};
pub use tool_injector::{get_sdk_tool_names, get_sdk_tools, inject_sdk_tools};
pub use types::{
    AggregationResult, RetrievalStrategy, SpawnedAgent, SpawnedAgentStatus, SpecialistType,
    SubAgentMemory, TaskRoute, rerank_facts_by_query,
};
