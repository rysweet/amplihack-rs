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

pub mod claude_md;
pub mod cleanup;
pub mod defensive;
pub mod process;
pub mod project_init;
pub mod slugify;

// Re-export the most commonly used items at crate root.
pub use defensive::{parse_llm_json, validate_json_schema};
pub use process::{CommandResult, ProcessManager};
pub use slugify::slugify;
