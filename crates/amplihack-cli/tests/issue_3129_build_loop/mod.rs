//! #3129 PR 2: the build goal loop and the old tools it keeps alive.
//!
//! These tests run the bundle's tools as they are called at step time,
//! against a stand-in `gh` and real git repositories under the test's own
//! temporary folder. None reads recipe, document or prompt text. Linux only;
//! no sleeps, timers or retries.
//!
//! - `claim_check`: every claim result exits 0 and the claim is information.
//! - `issue_reference`: how the run finds its issue, and from where.
//! - `host_inputs`: git-ignored `.venv` folders reach a new worktree (#3102).
//! - `pr_ready`: a finished run's own PR is marked ready (#2787).
//! - `goal_tool`: goal contract v1 for `autodrive_build_goal.sh`.
//! - `old_tools`: 1.2.213 callers of the changed tools still work (AC17).
//! - `pr_text`: the read-back and single correction of PR text.
//! - `quick_change`: quick-change keeps the task's PR lines (AC8).
//! - `guidance`: operator guidance and the PR-text allow list (A1).
//! - `run_script`: the build run script's loop exit table and report outcomes (A2 to A4).
//! - `routing`: post-loop steps skip repeated checks (AC14).
//! - `loop_drive`: the build goal through the real shared loop (AC6, AC9, AC13).

mod claim_check;
mod goal_tool;
mod guidance;
mod host_inputs;
mod issue_reference;
mod loop_drive;
mod old_tools;
mod pr_ready;
mod pr_text;
mod quick_change;
mod routing;
mod run_script;
pub(crate) mod tools;
