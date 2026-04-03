//! Goal-to-agent pipeline: prompt analysis, planning, skill synthesis, assembly.
//!
//! The pipeline transforms a raw user prompt into a deployable agent bundle:
//!
//! ```text
//! prompt ─► PromptAnalyzer ─► ObjectivePlanner ─► SkillSynthesizer
//!                                                       │
//!                          GoalAgentPackager ◄─ AgentAssembler
//! ```

pub mod analyzer;
pub mod assembler;
pub mod error;
pub mod models;
pub mod packager;
pub mod planner;
pub mod synthesizer;

pub use analyzer::PromptAnalyzer;
pub use assembler::AgentAssembler;
pub use error::{GeneratorError, Result};
pub use models::{
    BundleStatus, Complexity, ExecutionPlan, GenerationMetrics, GoalAgentBundle, GoalDefinition,
    PlanPhase, SDKToolConfig, SkillDefinition, SubAgentConfig,
};
pub use packager::GoalAgentPackager;
pub use planner::ObjectivePlanner;
pub use synthesizer::SkillSynthesizer;
