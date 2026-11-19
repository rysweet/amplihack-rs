//! Issues #2343, #1726 and #2809: `validate-agentic-finalization` reports what
//! the run actually produced (branch pushed at the local head, PR open at that
//! head, its check class) instead of failing on bookkeeping artefacts such as a
//! missing context file or unset implementation/verification booleans.
//!
//! Drives the real `workflow_agentic_finalization.sh validate` under
//! `env_clear()` with a private HOME, a local bare repo standing in for the
//! GitHub remote (via the #1641 `insteadOf` rewrite) and a stub `gh`. No
//! network, no timers, no retries. Gaps are asserted by the SHAs, branch and PR
//! number taken from the fixture at runtime, never by a fixed sentence.
#[path = "issue_2343_finalization_outcome/assessment_skip.rs"]
mod assessment_skip;
#[path = "issue_2343_finalization_outcome/classification.rs"]
mod classification;
#[path = "issue_2343_finalization_outcome/fixture_ext.rs"]
pub(crate) mod fixture_ext;
#[path = "issue_2343_finalization_outcome/security.rs"]
mod security;
