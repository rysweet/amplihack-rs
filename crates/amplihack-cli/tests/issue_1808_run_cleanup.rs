//! Issue #1808: recipe runs write into one work root, clean up after
//! themselves, and `amplihack cleanup` removes or archives what is left.
//!
//! The specification is `docs/howto/clean-up-after-recipe-runs.md`. These
//! tests run the real `amplihack` binary through `isolated(..)` with a temp
//! `HOME`, a temp `AMPLIHACK_WORK_ROOT`, a temp `TMPDIR` and a `PATH` that
//! holds only symlinked tools, a fake `gh` and a stub recipe runner (see
//! `fixture.rs`). No test reads the real `~/.claude`, `~/.amplihack`, profile
//! root or tmux server.

#![cfg(target_os = "linux")]

#[path = "issue_1808_run_cleanup/command.rs"]
mod command;
#[path = "issue_1808_run_cleanup/fixture.rs"]
mod fixture;
#[path = "issue_1808_run_cleanup/logs.rs"]
mod logs;
#[path = "issue_1808_run_cleanup/runner.rs"]
mod runner;
#[path = "issue_1808_run_cleanup/scripts.rs"]
mod scripts;
#[path = "issue_1808_run_cleanup/sweep.rs"]
mod sweep;
