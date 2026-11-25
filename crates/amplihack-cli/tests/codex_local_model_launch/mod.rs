//! Local-model launches of `amplihack codex` (no inserted hook text and a
//! small Codex home), at the binary's boundary.
//!
//! - `launch`: Codex arguments, environment and generated home;
//! - `errors`: invalid inputs and unsupported launches;
//! - `refresh`: user-home isolation, refresh and nested reuse checks.
//!
//! They share `harness`, modelled on `issue_1985_codex_agent_step_flags.rs`.
//! Unix only, like local-model launches themselves.
#![cfg(unix)]

mod errors;
pub(crate) mod harness;
pub(crate) mod launch;
mod native_trust;
mod refresh;
