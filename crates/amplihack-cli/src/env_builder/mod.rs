//! Type-safe environment builder for launching child processes.
//!
//! Constructs the environment variables needed by launched tools,
//! including AMPLIHACK_* vars and PATH augmentation. Uses set-based
//! PATH deduplication instead of error-prone substring matching.

mod builder;
mod helpers;

pub use builder::EnvBuilder;
pub use helpers::active_agent_binary;

#[cfg(test)]
mod tests_builder;
#[cfg(test)]
mod tests_vars;
