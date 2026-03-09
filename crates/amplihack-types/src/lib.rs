//! Thin IPC boundary types for amplihack hooks.
//!
//! Only types that cross process boundaries live here.
//! Domain types live in their domain crates.

pub mod hook_io;
pub mod settings;
pub mod tool_decision;

pub use hook_io::{HookInput, HookOutput, HookOutputDecision};
pub use settings::Settings;
pub use tool_decision::ToolDecision;
