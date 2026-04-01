//! amplihack-safety: Prevent data loss during auto mode and session starts.
//!
//! Provides git conflict detection, safe copy strategies, and prompt
//! transformation — matching the Python amplihack safety/ module.

pub mod conflict_detector;
pub mod copy_strategy;
pub mod prompt_transformer;

pub use conflict_detector::{ConflictDetectionResult, GitConflictDetector};
pub use copy_strategy::{CopyStrategy, SafeCopyStrategy};
pub use prompt_transformer::PromptTransformer;
