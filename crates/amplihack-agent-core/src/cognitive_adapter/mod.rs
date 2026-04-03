//! Cognitive adapter — unified memory interface over 6-type cognitive memory.
//!
//! Ports Python `amplihack/agents/goal_seeking/cognitive_adapter.py`.
//!
//! # Architecture
//!
//! `CognitiveAdapter` wraps a [`CognitiveBackend`] (either full 6-type
//! cognitive memory or hierarchical fallback) and an optional [`HiveStore`]
//! for distributed fact federation.
//!
//! It implements `MemoryRetriever` and `MemoryFacade` from the agentic loop
//! traits, making it a drop-in replacement in any loop that expects those
//! interfaces.
//!
//! # Module layout
//!
//! | File             | Responsibility                                    |
//! |------------------|---------------------------------------------------|
//! | `constants.rs`   | Tuning constants, stop-word set                   |
//! | `scoring.rs`     | Stop-word filtering, n-gram scoring, merge/dedup  |
//! | `types.rs`       | Backend / HiveStore / QualityScorer traits, types |
//! | `adapter.rs`     | `CognitiveAdapter` struct and trait impls          |

pub mod adapter;
pub mod constants;
pub mod scoring;
pub mod types;

#[cfg(test)]
mod tests;

pub use adapter::CognitiveAdapter;
pub use constants::{
    DEFAULT_CONFIDENCE_GATE, DEFAULT_QUALITY_THRESHOLD, FALLBACK_SCAN_MULTIPLIER,
    MAX_WORKING_SLOTS, SEARCH_CANDIDATE_MULTIPLIER,
};
pub use types::{
    BackendKind, CognitiveAdapterConfig, CognitiveBackend, HiveFact, HiveStore, Procedure,
    ProspectiveTrigger, QualityScorer, WorkingSlot,
};
