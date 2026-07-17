//! # amplihack-utils
//!
//! Foundational utilities for the amplihack ecosystem.
//!
//! ## Modules
//!
//! - [`slugify`](mod@slugify) — URL-safe slug generation with Unicode normalization
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
//! - [`bundle_generator`] — Agent bundle generation, packaging, and distribution
//! - [`docker_manager`] — Docker container management for isolated execution
//! - [`idle_watchdog`] — Idle/liveness watchdog supervising child processes (issue #867)

/// Single source of truth for resolving the active agent binary identifier.
pub mod agent_binary;
pub mod artifact_guard;
pub mod bundle_generator;
pub mod claude_cli;
pub mod claude_md;
pub mod cleanup;
pub mod defensive;
pub mod docker_detector;
pub mod docker_manager;
pub mod hook_merge;
/// Idle/liveness watchdog for supervising long-running child processes.
/// See issue #867.
pub mod idle_watchdog;
pub mod kb_types;
pub mod knowledge_builder;
pub mod litellm_callbacks;
pub mod llm_client;
pub mod observability;
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
/// Subprocess prompt-delivery helper (argv/tempfile/stdin). See Simard
/// issue #1897. STUB module in the TDD red phase.
pub mod prompt_delivery;
/// Secure file and directory creation with restrictive permissions.
pub mod secure_files;
pub mod send_input_allowlist;
pub mod settings_generator;
pub(crate) mod settings_helpers;
pub mod simple_tui;
pub(crate) mod simple_tui_runner;
pub mod slugify;
pub mod terminal_launcher;
pub mod trace_logger;
pub mod uvx_manager;
pub mod worktree;

// Re-export the most commonly used items at crate root.
pub use defensive::{
    ParseLlmJsonError, parse_llm_json, parse_llm_json_result, validate_json_schema,
};
pub use process::{CommandResult, ProcessManager};
pub use slugify::slugify;

/// Crate-wide serial lock for tests that mutate process-global state
/// (environment variables, current directory). Tests across different
/// modules share process-global state, so a per-module lock is insufficient
/// to prevent races under `cargo test --jobs N`. All such tests must acquire
/// this single shared lock.
#[cfg(test)]
pub(crate) mod test_serial {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Acquire the crate-wide test serialization guard. Poisoned locks are
    /// recovered so a panicking test does not cascade into false failures.
    pub(crate) fn acquire() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
