//! #3129: the goal loop. See each module's header for what it holds.
//!
//! Commit A (shared module): `contract`, `guidance_cli`.
//! Later commits: `goal_tool` (the auto-drive goal), `drive` (nine behaviour
//! tests), `turn_failures` (R1: a failed turn run does not end the run) and
//! `required_outcomes` (R2: outcomes checked by name before a merge). They
//! share `support`. Linux only; no sleeps, timers or timeouts.

mod contract;
mod drive;
mod goal_tool;
mod guidance_cli;
mod required_outcomes;
pub(crate) mod support;
mod turn_failures;
