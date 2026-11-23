//! #3129: the goal loop. See each module's header for what it holds.
//!
//! Commit A (shared module): `contract`, `guidance_cli`.
//! Later commits: `goal_tool` (the auto-drive goal), `drive` (nine behaviour
//! tests). They share `support`. Linux only; no sleeps, timers or timeouts.

mod contract;
mod drive;
mod goal_tool;
mod guidance_cli;
pub(crate) mod support;
