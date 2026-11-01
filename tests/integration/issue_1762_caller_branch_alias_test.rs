//! Issue #1762: when the target branch is checked out in the caller's own
//! checkout, default-workflow step 4 no longer stops with the #858 refusal.
//! It creates the task worktree on a local alias, `<B>--wt-<n>`, that tracks
//! origin/<B>, and every later step that pushes or looks up the PR uses <B>.
//! The caller's checkout is never touched, and a caller whose checkout holds
//! work the task worktree would not see stops with a message saying so.
//!
//! - `step04.rs`: the alias path: creation, reuse, skipping, a caller that is
//!   behind or a linked worktree, and a caller not on <B> (unchanged).
//! - `step04_stops.rs`: the three stops, first cause only, the default
//!   branch, a missing or misbehaving planner, and a concurrent create.
//! - `resolver.rs`, `planner.rs`: the resolver and planner on their own;
//!   `resolver.rs` also holds the bash 3.2 and strict-mode checks for all
//!   three new tools (resolver, planner and `workflow_alias_reuse.sh`).
//! - `consumers.rs`: the open-PR check, final status and the terminal-state
//!   probe look the PR up by <B> (AC11).
//! - `push.rs`: steps 15, 16b, 18c and 20b publish to origin/<B>.
//! - `static_checks.rs`: the planner and resolver only read, the reuse
//!   helper's only write is one fast-forward, step 4 and the docs.
//!
//! Every child starts from `isolated(..)` with a fixture-owned HOME, a fake
//! `gh` and a bare local origin; nothing reaches GitHub or the real
//! `~/.claude`, `~/.amplihack` or profile root. See `fixture.rs`.
#[path = "issue_1762_caller_branch_alias/consumers.rs"]
mod consumers;
#[path = "issue_1762_caller_branch_alias/fixture.rs"]
mod fixture;
#[path = "issue_1762_caller_branch_alias/planner.rs"]
mod planner;
#[path = "issue_1762_caller_branch_alias/push.rs"]
mod push;
#[path = "issue_1762_caller_branch_alias/resolver.rs"]
mod resolver;
#[path = "issue_1762_caller_branch_alias/static_checks.rs"]
mod static_checks;
#[path = "issue_1762_caller_branch_alias/step04.rs"]
mod step04;
#[path = "issue_1762_caller_branch_alias/step04_stops.rs"]
mod step04_stops;
