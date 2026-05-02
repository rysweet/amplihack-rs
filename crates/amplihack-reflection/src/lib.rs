//! amplihack-reflection: state machine, semaphore, security sanitization,
//! lightweight analyzer, semantic duplicate detector, error analysis, and
//! the high-level reflection orchestrator.
//!
//! Native Rust port of `amplifier-bundle/tools/amplihack/reflection/*.py`.

pub mod display;
pub mod error_analysis;
pub mod lightweight_analyzer;
pub mod reflection;
pub mod security;
pub mod semantic_duplicate_detector;
pub mod semaphore;
pub mod state_machine;
