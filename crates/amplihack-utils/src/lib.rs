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
//! - [`plugin_manifest`] — Plugin manifest types and validation
//! - [`plugin_manager`] — Plugin installation, uninstallation, and path resolution
//! - [`plugin_cli`] — CLI command handlers for plugin management
//! - [`simple_tui`] — Simple TUI testing framework with gadugi and subprocess fallback
//! - [`knowledge_builder`] — Knowledge Builder orchestrator (Socratic method pipeline)
//! - [`llm_client`] — SDK launcher detection and LLM completion routing

pub mod claude_cli;
pub mod claude_md;
pub mod cleanup;
pub mod defensive;
pub mod docker_detector;
pub mod kb_types;
pub mod knowledge_builder;
pub mod llm_client;
pub mod plugin_cli;
pub mod plugin_manager;
pub(crate) mod plugin_manager_paths;
pub mod plugin_manifest;
pub mod plugin_verifier;
pub mod power_steering;
pub mod prerequisites;
pub mod process;
pub mod project_init;
pub(crate) mod project_init_detect;
pub mod send_input_allowlist;
pub mod settings_generator;
pub(crate) mod settings_helpers;
pub mod simple_tui;
pub(crate) mod simple_tui_runner;
pub mod slugify;
pub mod terminal_launcher;
pub mod trace_logger;
pub mod uvx_manager;
pub mod hook_merge;
pub mod worktree;

// Re-export the most commonly used items at crate root.
pub use defensive::{parse_llm_json, validate_json_schema};
pub use process::{CommandResult, ProcessManager};
pub use slugify::slugify;
