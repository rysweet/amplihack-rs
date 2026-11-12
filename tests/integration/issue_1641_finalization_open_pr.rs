//! Issue #1641: `validate-agentic-finalization` must accept a resumed run whose
//! branch already has a published, open, same-repo PR with a passing
//! verification signal (all checks green, or CI not yet reporting plus this
//! run's typed precommit evidence) as `EXISTING_OPEN_PR`, consistent with
//! step-22b since #1623, while keeping every fail-closed case: no PR, no
//! verification signal, a failing check (`BLOCKED_CI`), hollow success, dirty
//! worktree, prior blockers and reporting failures.
//!
//! Runs the real `workflow_agentic_finalization.sh validate` against a private
//! git repo and a stub `gh`; needs bash, git and jq, never the network.
#[path = "issue_1641_finalization_open_pr/blocked_ci.rs"]
mod blocked_ci;
#[path = "issue_1641_finalization_open_pr/builders.rs"]
pub(crate) mod builders;
#[path = "issue_1641_finalization_open_pr/contracts.rs"]
mod contracts;
#[path = "issue_1641_finalization_open_pr/fail_closed.rs"]
mod fail_closed;
#[path = "issue_1641_finalization_open_pr/fixture.rs"]
pub(crate) mod fixture;
#[path = "issue_1641_finalization_open_pr/head_and_fallback.rs"]
mod head_and_fallback;
#[path = "issue_1641_finalization_open_pr/metadata.rs"]
mod metadata;
#[path = "issue_1641_finalization_open_pr/signal.rs"]
mod signal;
#[path = "issue_1641_finalization_open_pr/success.rs"]
mod success;
