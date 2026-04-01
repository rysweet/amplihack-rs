//! amplihack-launcher: Extended launcher support for all agent types.
//!
//! Provides Codex, Amplifier, and Copilot MCP launchers, plus session
//! forking and transcript capture — matching the Python amplihack launcher
//! subsystem.

pub mod amplifier;
pub mod codex;
pub mod copilot_mcp;
pub mod fork_manager;
pub mod session_capture;

pub use amplifier::AmplifierInfo;
pub use codex::CodexInfo;
pub use copilot_mcp::McpServerConfig;
pub use fork_manager::{ForkConfig, ForkDecision, ForkManager};
pub use session_capture::{CapturedMessage, MessageCapture, MessageRole};
