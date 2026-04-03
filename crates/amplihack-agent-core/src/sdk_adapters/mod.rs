//! SDK adapter system for goal-seeking agents.
//!
//! Ports the Python `amplihack/agents/goal_seeking/sdk_adapters/` package.
//!
//! # Architecture
//!
//! The adapter pattern decouples the agent logic from the specific SDK used
//! to communicate with LLMs. Each adapter wraps an [`SdkClient`] and exposes
//! a uniform [`SdkAdapter`] interface.
//!
//! ```text
//! ┌─────────────┐
//! │   Factory    │  create_adapter(config) → Box<dyn SdkAdapter>
//! └──────┬──────┘
//!        │ selects
//!        ▼
//! ┌─────────────┐     ┌─────────────┐     ┌─────────────────┐
//! │   Claude     │     │   Copilot   │     │   Microsoft     │
//! │   Adapter    │     │   Adapter   │     │   Adapter       │
//! └──────┬──────┘     └──────┬──────┘     └────────┬────────┘
//!        │                   │                      │
//!        ▼                   ▼                      ▼
//!   Box<dyn SdkClient>  Box<dyn SdkClient>   Box<dyn SdkClient>
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use amplihack_agent_core::sdk_adapters::{
//!     create_adapter, SdkAdapterConfig, SdkType,
//! };
//!
//! let config = SdkAdapterConfig::new("my-agent", SdkType::Claude)
//!     .with_model("claude-3-sonnet")
//!     .with_instructions("Be precise and concise");
//!
//! let mut adapter = create_adapter(config).unwrap();
//! adapter.create_agent().unwrap();
//! ```

pub mod base;
pub mod claude;
pub mod copilot;
pub mod factory;
pub mod microsoft;
pub mod types;

// Re-exports for ergonomic access.
pub use base::{SdkAdapter, SdkClient, SdkClientResponse};
pub use claude::ClaudeAdapter;
pub use copilot::CopilotAdapter;
pub use factory::{create_adapter, create_adapter_by_name};
pub use microsoft::MicrosoftAdapter;
pub use types::{
    AdapterResult, AgentTool, Goal, SdkAdapterConfig, SdkType, ToolCategory,
};
