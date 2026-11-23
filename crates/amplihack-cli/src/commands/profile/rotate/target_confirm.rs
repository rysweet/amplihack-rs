//! The pre-swap confirmation step (issue #2910, D6): for a stale target a
//! decision chose, whether it is read again, or rejected so that the decision
//! runs again and reaches the next-ranked login. A target that is not stale
//! needs no step: its reading stands. Pure: the freshness gate
//! (`fresh::settle`) gathers the facts, makes any read and writes the lines.
//!
//! A target is swapped to only on a fresh reading, whatever room the live
//! login has (issue #3105, which withdraws #2910's D7): a target whose probe
//! failed, or whose confirming read fails, comes back unknown or is older
//! than a reading already held, is rejected and never taken unconfirmed.
//!
//! | The stale target | Step |
//! |------------------|------|
//! | this cycle's re-reads are used up | [`Step::NotTried`] ([`Limit::Count`]) |
//! | its probe failed this cycle | [`Step::Reject`] ([`Why::Unknown`], or [`Why::Unfit`] with no label) |
//! | the cycle can read no more | [`Step::NotTried`] ([`Limit::Time`]) |
//! | it can be read | [`Step::ReadAgain`] |
//!
//! A probe that failed this cycle is never read again in it: there is
//! nothing to read, only to decide. A rejection made without a read counts
//! against the cap of [`MAX_REREADS_PER_CYCLE`], so a cycle writes at most
//! three such `reject:` lines (E5). No retry loop, backoff or timer is added:
//! the gate's existing cap and shared time budget bound the step.

use super::super::fresh::MAX_REREADS_PER_CYCLE;
use super::super::measure::ProbeLabel;
use super::target::Unfit;

/// What the gate knows about the chosen target, whose reading is stale by
/// `fresh::stale` (no reading time, too old, older than the live reading, or
/// not after a damped swap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// Its probe failed in this cycle (failed and not carried from an
    /// earlier cycle; a read still running counts).
    pub failed_now: bool,
    /// Re-reads made, and rejections made without a read, in this cycle.
    pub attempts: usize,
    /// The gate's shared time budget still has time for a read.
    pub can_read: bool,
    /// The label of the target's probe, when it was unknown.
    pub label: Option<ProbeLabel>,
}

/// Why the gate stopped short of a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// The cycle's re-reads are used up.
    Count,
    /// The gate's time budget is spent.
    Time,
}

/// Why a candidate was rejected at confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Its reading is unknown.
    Unknown(ProbeLabel),
    /// Its fresh reading makes it ineligible.
    Unfit(Unfit),
    /// Its confirming read is this many seconds older than a reading already
    /// held for it (issue #3105).
    Older(u64),
}

/// What the gate does with the chosen target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Read it again and decide on the new reading.
    ReadAgain,
    /// Reject it and decide again without it.
    Reject(Why),
    /// Make it unknown for this cycle without a read.
    NotTried(Limit),
}

/// The step for one stale chosen target (D6, issue #3105).
pub fn next(f: &Facts) -> Step {
    if f.attempts >= MAX_REREADS_PER_CYCLE {
        return Step::NotTried(Limit::Count);
    }
    if f.failed_now {
        return Step::Reject(match f.label {
            Some(label) => Why::Unknown(label),
            // A failed probe with no label has no last good reading, so it
            // never ranks; should one be chosen, it has nothing to stand on.
            None => Why::Unfit(Unfit::NoReading),
        });
    }
    if !f.can_read {
        return Step::NotTried(Limit::Time);
    }
    Step::ReadAgain
}

#[cfg(test)]
#[path = "tests_issue_2910_unknown.rs"]
mod tests_issue_2910_unknown;
