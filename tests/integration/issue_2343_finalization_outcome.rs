//! Issues #2343, #1726 and #2809: finalization reports what the run actually
//! produced instead of failing on bookkeeping artefacts such as a missing
//! context file or unset implementation/verification booleans.
//!
//! The work is split the way the code is. `facts.rs` drives
//! `finalization_outcome_facts`, which collects what git and gh printed and
//! decides nothing. `terminal_states.rs` drives the whole
//! `workflow_agentic_finalization.sh validate` step and asserts the end state
//! it reports for the terminal state `assess-finalization` decided.
//! `recovery.rs` covers the case with no decision at all, and `security.rs`
//! the trust boundary around everything the step is handed.
//!
//! Everything runs under `env_clear()` with a private HOME, a local bare repo
//! standing in for the GitHub remote (via the #1641 `insteadOf` rewrite) and a
//! stub `gh`. No network, no timers, no retries. Gaps are asserted by the
//! SHAs, branch and PR number taken from the fixture at runtime, never by a
//! fixed sentence.
#[path = "issue_2343_finalization_outcome/facts.rs"]
mod facts;
#[path = "issue_2343_finalization_outcome/fixture_ext.rs"]
pub(crate) mod fixture_ext;
#[path = "issue_2343_finalization_outcome/recovery.rs"]
mod recovery;
#[path = "issue_2343_finalization_outcome/security.rs"]
mod security;
#[path = "issue_2343_finalization_outcome/terminal_states.rs"]
mod terminal_states;
