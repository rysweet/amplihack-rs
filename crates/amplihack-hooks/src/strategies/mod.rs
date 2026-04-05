//! Host-specific hook strategies.
//!
//! Each supported launcher implements [`HookStrategy`] to customise
//! context injection and power-steering dispatch.

pub mod base;
pub mod claude_strategy;
pub mod copilot_strategy;

pub use base::HookStrategy;
pub use claude_strategy::ClaudeStrategy;
pub use copilot_strategy::CopilotStrategy;
