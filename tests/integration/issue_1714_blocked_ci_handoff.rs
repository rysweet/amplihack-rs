//! Issue #1714: inside auto-drive, a `default-workflow` build whose
//! finalization ends `BLOCKED_CI` with a published, open PR hands that PR to
//! the crusty loop and the merge-ready loop instead of ending the run. A
//! standalone `default-workflow` still fails closed on `BLOCKED_CI`, and every
//! other finalization failure still ends auto-drive.
//!
//! The hand-off is decided from typed data only (`terminal_state`, `handoff`
//! and the PR number), defined once in
//! `amplifier-bundle/tools/workflow_blocked_ci_handoff.sh`, switched on only by
//! `RECIPE_VAR_blocked_ci_handoff` being exactly `true`, which only
//! autodrive-build step-02 sets.
//!
//! Every child is started through `isolated(..)` with a private HOME, a stub
//! `gh` and (for step-03) a stub `amplihack`; nothing reaches GitHub or the
//! real `~/.claude`, `~/.amplihack` or profile root.
#[path = "issue_1714_blocked_ci_handoff/brick.rs"]
mod brick;
#[path = "issue_1714_blocked_ci_handoff/fixture.rs"]
mod fixture;
#[path = "issue_1714_blocked_ci_handoff/static_guards.rs"]
mod static_guards;
#[path = "issue_1714_blocked_ci_handoff/step03.rs"]
mod step03;
#[path = "issue_1714_blocked_ci_handoff/validator.rs"]
mod validator;
