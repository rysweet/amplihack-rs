//! #3129: the goal loop. See each module's header for what it holds.
//!
//! Commit A (shared module): `contract`, `guidance_cli`.
//! Later commits: `goal_tool` (the auto-drive goal), `drive` (nine behaviour
//! tests), `turn_failures` (R1: a failed turn run does not end the run),
//! `required_outcomes` (R2: outcomes checked by name before a merge) and
//! `kept_description` (the evidence goes in a comment when the task keeps the
//! description, and the run merges). They share `support`. Linux only; no
//! sleeps, timers or timeouts.

mod checks_watch_interval;
pub(crate) mod ci_wait;
mod ci_wait_conflict;
mod codex_limit_wait;
mod contract;
mod drive;
mod goal_tool;
mod guidance_cli;
mod kept_description;
mod minor_findings;
mod required_outcomes;
pub(crate) mod support;
mod turn_failures;
