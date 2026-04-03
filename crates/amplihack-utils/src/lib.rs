//! # amplihack-utils
//!
//! Foundational utilities for the amplihack ecosystem.
//!
//! ## Modules
//!
//! - [`slugify`] — URL-safe slug generation with Unicode normalization
//! - [`defensive`] — LLM response parsing, file I/O retry, prompt isolation
//! - [`process`] — Cross-platform process management with timeout support
//! - [`project_init`] — Project initialization and `PROJECT.md` management
//! - [`claude_md`] — `CLAUDE.md` preservation and version management
//! - [`cleanup`] — Cleanup registry and handler for tracked temporary paths
//! - [`claude_cli`] — Claude CLI binary detection, installation, version checking
//! - [`prerequisites`] — Tool detection and installation guidance
//! - [`worktree`] — Git worktree detection and shared runtime directory resolution
//! - [`settings_generator`] — Plugin settings generation, merging, and writing
//! - [`power_steering`] — Power-steering re-enable prompt with timeout

pub mod claude_cli;
pub mod claude_md;
pub mod cleanup;
pub mod defensive;
pub mod docker_detector;
pub mod kb_types;
pub mod plugin_verifier;
pub mod power_steering;
pub mod prerequisites;
pub mod process;
pub mod project_init;
pub(crate) mod project_init_detect;
pub mod send_input_allowlist;
pub mod settings_generator;
pub(crate) mod settings_helpers;
pub mod slugify;
pub mod trace_logger;
pub mod worktree;

// Re-export the most commonly used items at crate root.
pub use defensive::{parse_llm_json, validate_json_schema};
pub use process::{CommandResult, ProcessManager};
pub use slugify::slugify;
