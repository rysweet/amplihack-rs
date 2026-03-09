//! Hook implementations for amplihack.
//!
//! All hooks are host-agnostic — they work with Claude Code, Amplifier,
//! and Copilot via JSON stdin/stdout protocol.

pub mod post_tool_use;
pub mod pre_compact;
pub mod pre_tool_use;
pub mod protocol;
pub mod session_start;
pub mod session_stop;
pub mod stop;
pub mod user_prompt;

// Re-export hook structs for ergonomic access.
pub use post_tool_use::PostToolUseHook;
pub use pre_compact::PreCompactHook;
pub use pre_tool_use::PreToolUseHook;
pub use protocol::{FailurePolicy, Hook};
pub use session_start::SessionStartHook;
pub use session_stop::SessionStopHook;
pub use stop::StopHook;
pub use user_prompt::UserPromptSubmitHook;
