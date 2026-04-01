//! amplihack-recipe: Recipe system extensions for parity with Python.
//!
//! Provides agent resolution, recipe discovery/caching, branch name
//! sanitization, and sub-recipe recovery — filling gaps between the
//! existing Rust recipe runner and the Python recipe subsystem.

pub mod agent_resolver;
pub mod branch_name;
pub mod discovery;
pub mod sub_recipe_recovery;

pub use agent_resolver::AgentResolver;
pub use branch_name::{make_branch_name, sanitize_branch_name};
pub use discovery::{RecipeCache, RecipeInfo, discover_recipes, find_recipe, list_recipes};
pub use sub_recipe_recovery::{FailureClass, FailureContext, RecoveryResult, SubRecipeRecovery};
