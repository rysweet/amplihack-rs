//! Type-safe environment builder for launching child processes.
//!
//! Constructs the environment variables needed by launched tools,
//! including AMPLIHACK_* vars and PATH augmentation. Uses set-based
//! PATH deduplication instead of error-prone substring matching.
//!
//! `AMPLIHACK_PARENT_EFFORT` and `AMPLIHACK_PARENT_MODEL` (issue #1883) are
//! not managed here: they are set after `EnvBuilder` is applied, by
//! `launch::inherit::export_to_child` and by `recipe run` for its runner.

pub mod agent_binary_resolver;
mod builder;
pub mod helpers;

pub use builder::EnvBuilder;
pub use helpers::{
    active_agent_binary, launch_binary_source, launched_on_a_default_guess, resolve_agent_binary_in,
};

#[cfg(test)]
mod tests_builder;
#[cfg(test)]
mod tests_vars;
