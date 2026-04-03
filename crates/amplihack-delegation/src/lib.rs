//! Meta-delegation orchestration for amplihack.
//!
//! Ports the Python `meta_delegation` module, providing persona-driven
//! delegation, subprocess state tracking, scenario generation, and
//! evidence-based success evaluation.

/// Error types for delegation operations.
pub mod error;
/// Evidence collection from working directories.
pub mod evidence_collector;
/// Data models: results, evidence, scenarios.
pub mod models;
/// Persona strategies and registry.
pub mod persona;
/// Platform CLI abstraction for spawning AI-assistant subprocesses.
pub mod platform_cli;
/// Test-scenario generation from goals and criteria.
pub mod scenario;
/// Subprocess state machine with validated transitions.
pub mod state_machine;
/// Success-criteria evaluation against collected evidence.
pub mod success_evaluator;

pub use error::DelegationError;
pub use evidence_collector::EvidenceCollector;
pub use models::{
    DelegationStatus, EvaluationResult, EvidenceItem, EvidenceType, MetaDelegationResult,
    ScenarioCategory, SubprocessResult, TestScenario,
};
pub use persona::{PersonaStrategy, get_persona, register_persona};
pub use platform_cli::{
    AmplifierCli, ClaudeCodeCli, CopilotCli, PlatformCli, SpawnConfig, available_platforms,
    get_platform,
};
pub use scenario::ScenarioGenerator;
pub use state_machine::{ProcessState, SubprocessStateMachine};
pub use success_evaluator::SuccessEvaluator;
