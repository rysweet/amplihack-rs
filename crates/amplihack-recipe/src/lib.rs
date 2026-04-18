//! amplihack-recipe: Recipe system extensions for parity with Python.
//!
//! Provides recipe models, YAML parsing, template expansion, recipe execution,
//! agent resolution, recipe discovery/caching, branch name sanitization,
//! and sub-recipe recovery.

pub mod agent_resolver;
pub mod branch_name;
pub mod condition_eval;
pub mod discovery;
pub mod executor;
pub mod models;
pub mod parser;
pub mod progress_validator;
pub mod sub_recipe_recovery;
pub mod template;

pub use agent_resolver::{AgentDefinition, AgentMetadata, AgentResolver};
pub use branch_name::{make_branch_name, sanitize_branch_name};
pub use condition_eval::{ConditionError, evaluate_condition, validate_condition};
pub use discovery::{RecipeCache, RecipeInfo, discover_recipes, find_recipe, list_recipes};
pub use executor::{
    AgentBackend, DryRunAgentBackend, ExecutorConfig, RecipeExecutor, context_from_env,
    merge_context,
};
pub use models::{
    ContextValidationRule, Recipe, RecipeResult, RecursionConfig, Step, StepResult, StepStatus,
    StepType,
};
pub use parser::RecipeParser;
pub use progress_validator::{
    ProgressPayload, ProgressStatus, ValidationError, WorkstreamState, atomic_write_json,
    merge_workstream_state_into_progress, progress_file_path, read_progress_file,
    read_workstream_state, validate_path_within_tmpdir, validate_progress_file,
    workstream_progress_sidecar_path, workstream_state_file_path,
};
pub use sub_recipe_recovery::{FailureClass, FailureContext, RecoveryResult, SubRecipeRecovery};
pub use template::{expand_step_strings, expand_template};
