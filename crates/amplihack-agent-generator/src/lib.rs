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
pub mod distributor;
pub mod documentation_generator;
pub mod error;
pub mod models;
pub mod packager;
pub mod planner;
pub mod repackage_generator;
pub mod repository_creator;
pub mod synthesizer;
pub mod update_manager;

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

// Re-exports for newly ported modules
pub use distributor::{DistributionResult, GitHubDistributor, PackageMeta};
pub use documentation_generator::{generate_instructions, BundleDocMeta};
pub use repackage_generator::{
    generate_bash_script, generate_python_script, make_executable, sanitize_bundle_name,
    sanitize_version,
};
pub use repository_creator::{RepositoryCreator, RepositoryResult};
pub use update_manager::{compute_checksum, UpdateInfo, UpdateManager, UpdateResult};
