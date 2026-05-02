//! Native Rust port of `amplifier-bundle/tools/amplihack/orchestration/`.
//!
//! Provides Claude subprocess execution, parallel/sequential/fallback
//! execution helpers, sessions with structured logging, and four high-level
//! orchestration patterns (n_version, debate, cascade, expert_panel).

pub mod claude_process;
pub mod claude_process_builder;
pub mod execution;
pub mod patterns;
pub mod session;

pub use claude_process_builder::ClaudeProcessBuilder;
