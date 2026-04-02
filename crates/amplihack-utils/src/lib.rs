//! # amplihack-utils
//!
//! Foundational utilities for the amplihack ecosystem.
//!
//! ## Modules
//!
//! - [`slugify`] — URL-safe slug generation with Unicode normalization
//! - [`defensive`] — LLM response parsing, file I/O retry, prompt isolation
//! - [`process`] — Cross-platform process management with timeout support

pub mod defensive;
pub mod process;
pub mod slugify;

// Re-export the most commonly used items at crate root.
pub use defensive::{parse_llm_json, validate_json_schema};
pub use process::{CommandResult, ProcessManager};
pub use slugify::slugify;
