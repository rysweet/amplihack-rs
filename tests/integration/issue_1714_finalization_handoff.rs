//! Issue #1714: inside auto-drive, a `default-workflow` build whose
//! finalization ends `BLOCKED_CI`, or `FAILED_IMPLEMENTATION` or
//! `FAILED_REPORTING` while the open-PR check confirmed a published, open PR
//! (`reporting_handoff.rs` for the reporting-failure exits), hands that PR to the crusty
//! loop and the merge-ready loop instead of ending the run. A standalone
//! `default-workflow` still fails closed, and every other finalization failure
//! still ends auto-drive. The open-PR check also accepts a PR head that is a
//! fast-forward of the local HEAD (case 2, `ancestor*.rs`), and so does the
//! terminal-state probe that workflow-finalize runs before it (`probe.rs`).
//! After that probe, `step-20-sync-pr-head` fast-forwards the worktree to
//! such a head and step-20b pulls before it pushes, with or without an
//! upstream (`step20b.rs`, fixture in `step20b_remote.rs`).
//!
//! The hand-off is decided from typed data only (`terminal_state`, `handoff`
//! and the PR number), defined once in
//! `amplifier-bundle/tools/workflow_finalization_handoff.sh`, switched on only by
//! `RECIPE_VAR_finalization_handoff` being exactly `true`, which only
//! autodrive-build step-02 sets.
//!
//! Every child is started through `isolated(..)` with a private HOME, a stub
//! `gh` and (for step-03) a stub `amplihack`; nothing reaches GitHub or the
//! real `~/.claude`, `~/.amplihack` or profile root.
#[path = "issue_1714_finalization_handoff/ancestor.rs"]
mod ancestor;
#[path = "issue_1714_finalization_handoff/ancestor_fetch_ssh.rs"]
mod ancestor_fetch_ssh;
#[path = "issue_1714_finalization_handoff/ancestor_handoff.rs"]
mod ancestor_handoff;
#[path = "issue_1714_finalization_handoff/brick.rs"]
mod brick;
#[path = "issue_1714_finalization_handoff/fixture.rs"]
mod fixture;
#[path = "issue_1714_finalization_handoff/probe.rs"]
mod probe;
#[path = "issue_1714_finalization_handoff/probe_stale.rs"]
mod probe_stale;
#[path = "issue_1714_finalization_handoff/reporting_handoff.rs"]
mod reporting_handoff;
#[path = "issue_1714_finalization_handoff/static_guards.rs"]
mod static_guards;
#[path = "issue_1714_finalization_handoff/step03.rs"]
mod step03;
#[path = "issue_1714_finalization_handoff/step03_failed_implementation.rs"]
mod step03_failed_implementation;
#[path = "issue_1714_finalization_handoff/step20b.rs"]
mod step20b;
#[path = "issue_1714_finalization_handoff/step20b_remote.rs"]
mod step20b_remote;
#[path = "issue_1714_finalization_handoff/validator.rs"]
mod validator;
#[path = "issue_1714_finalization_handoff/validator_routing.rs"]
mod validator_routing;
