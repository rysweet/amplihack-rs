//! Meta-delegation orchestration for amplihack.
//!
//! Ports the Python `meta_delegation` module, providing persona-driven
//! delegation, subprocess state tracking, scenario generation, and
//! evidence-based success evaluation.

/// Error types for delegation operations.
pub mod error;
/// Data models: results, evidence, scenarios.
pub mod models;
/// Persona strategies and registry.
pub mod persona;
/// Test-scenario generation from goals and criteria.
pub mod scenario;
/// Subprocess state machine with validated transitions.
pub mod state_machine;

pub use error::DelegationError;
pub use models::{
    DelegationStatus, EvidenceItem, EvidenceType, MetaDelegationResult, ScenarioCategory,
    SubprocessResult, TestScenario,
};
pub use persona::{PersonaStrategy, get_persona, register_persona};
pub use scenario::ScenarioGenerator;
pub use state_machine::{ProcessState, SubprocessStateMachine};
