//! Local-model launches of `amplihack codex` (no inserted hook text and a
//! small Codex home), at the binary's boundary.
//!
//! - `launch`: what Codex is started with and what the generated home holds
//!   (AC7, AC8, AC11, AC13, AC14, SEC-F6);
//! - `errors`: refused launches (AC12, SEC-L1, SEC-L6, SEC-D1, SEC-D2, SF2,
//!   SF6);
//! - `refresh`: the user's Codex home untouched (AC9), refresh (AC10),
//!   nested reuse and its checks (AC18, SEC-F5).
//!
//! They share `harness`, modelled on `issue_1985_codex_agent_step_flags.rs`.
//! Unix only, like local-model launches themselves (D21).
#![cfg(unix)]

mod errors;
pub(crate) mod harness;
pub(crate) mod launch;
mod refresh;
