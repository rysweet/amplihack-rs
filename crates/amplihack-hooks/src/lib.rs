//! Hook implementations for amplihack.
//!
//! All hooks are host-agnostic — they work with Claude Code, Amplifier,
//! and Copilot via JSON stdin/stdout protocol.

mod agent_memory;
mod original_request;
mod prompt_input;
#[cfg(test)]
pub(crate) mod test_support;

/// Post-tool-use hook: observes tool results for metrics and validation.
pub mod post_tool_use;
/// Pre-compact hook: exports conversation transcript before context compaction.
pub mod pre_compact;
/// Pre-tool-use hook: decides whether to allow or deny a tool invocation.
pub mod pre_tool_use;
/// Hook protocol traits and failure policies.
pub mod protocol;
/// Session start hook: initializes session state and injects context.
pub mod session_start;
/// Session stop hook: finalizes session state and exports data.
pub mod session_stop;
/// Stop hook: decides whether to allow or block session exit.
pub mod stop;
/// User prompt submission hook: processes user prompt before the LLM call.
pub mod user_prompt;
/// Workflow classification reminder hook: injects topic-boundary routing guidance.
pub mod workflow_classification;

// Re-export hook structs for ergonomic access.
/// Post-tool-use hook implementation.
pub use post_tool_use::PostToolUseHook;
/// Pre-compact hook implementation.
pub use pre_compact::PreCompactHook;
/// Pre-tool-use hook implementation.
pub use pre_tool_use::PreToolUseHook;
/// Hook protocol trait and failure policy enum.
pub use protocol::{FailurePolicy, Hook};
/// Session start hook implementation.
pub use session_start::SessionStartHook;
/// Session stop hook implementation.
pub use session_stop::SessionStopHook;
/// Stop hook implementation.
pub use stop::StopHook;
/// User prompt submission hook implementation.
pub use user_prompt::UserPromptSubmitHook;
/// Workflow classification reminder hook implementation.
pub use workflow_classification::WorkflowClassificationReminderHook;
