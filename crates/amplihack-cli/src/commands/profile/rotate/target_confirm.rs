//! The pre-swap confirmation step (issue #2910, D6 and D7): for the target a
//! decision chose, whether its reading stands, is read again, is accepted
//! without a fresh reading, or is rejected so that the decision runs again
//! and reaches the next-ranked login. Pure: the freshness gate
//! (`fresh::settle`) gathers the facts, makes any read and writes the lines.
//!
//! | The target | Step |
//! |------------|------|
//! | read directly this cycle and fresh | [`Step::Accept`], with no second read |
//! | stale, and this cycle's re-reads are used up | [`Step::NotTried`] ([`Limit::Count`]) |
//! | its probe failed this cycle, live login has no room now | [`Step::AcceptUnconfirmed`] |
//! | its probe failed this cycle, live login has room | [`Step::Reject`] ([`Why::Unknown`]) |
//! | stale, and the cycle can read no more | [`Step::NotTried`] ([`Limit::Time`]) |
//! | stale, and it can be read | [`Step::ReadAgain`] |
//!
//! A probe that failed this cycle is never read again in it: there is
//! nothing to read, only to decide. A rejection made without a read counts
//! against the cap of [`MAX_REREADS_PER_CYCLE`], so a cycle writes at most
//! three `reject:` lines (E5). No retry loop, backoff or timer is added: the
//! gate's existing cap and shared time budget bound the step.

use super::super::fresh::MAX_REREADS_PER_CYCLE;
use super::super::measure::ProbeLabel;
use super::target::Unfit;

/// What the gate knows about the chosen target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// Its reading is stale by `fresh::stale` (no reading time, too old,
    /// older than the live reading, or not after a damped swap).
    pub stale: bool,
    /// Its probe failed in this cycle (failed and not carried from an
    /// earlier cycle; a read still running counts).
    pub failed_now: bool,
    /// Re-reads made, and rejections made without a read, in this cycle.
    pub attempts: usize,
    /// The gate's shared time budget still has time for a read.
    pub can_read: bool,
    /// The live login has no room now (a window at 100%, or a block). Never
    /// true for an overflow live login (E5b).
    pub live_no_room: bool,
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
    /// Its reading is unknown, and the live login has room.
    Unknown(ProbeLabel),
    /// Its fresh reading makes it ineligible.
    Unfit(Unfit),
}

/// What the gate does with the chosen target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Its reading stands: swap.
    Accept,
    /// Swap on the reading the decision used, without a fresh one, and say
    /// so on the swap line.
    AcceptUnconfirmed(ProbeLabel),
    /// Read it again and decide on the new reading.
    ReadAgain,
    /// Reject it and decide again without it.
    Reject(Why),
    /// Make it unknown for this cycle without a read.
    NotTried(Limit),
}

/// The step for one chosen target (D6, D7).
pub fn next(f: &Facts) -> Step {
    if !f.stale {
        return Step::Accept;
    }
    if f.attempts >= MAX_REREADS_PER_CYCLE {
        return Step::NotTried(Limit::Count);
    }
    if f.failed_now {
        return match f.label {
            Some(label) => after_unknown(label, f.live_no_room),
            // A failed probe with no label has no last good reading, so it
            // never ranks; should one be chosen, it has nothing to stand on.
            None => Step::Reject(Why::Unfit(Unfit::NoReading)),
        };
    }
    if !f.can_read {
        return Step::NotTried(Limit::Time);
    }
    Step::ReadAgain
}

/// The step for a target whose reading is unknown (its probe failed this
/// cycle, or its re-read came back unknown): accepted without a fresh
/// reading only when the live login has no room now, else rejected (D7).
pub fn after_unknown(label: ProbeLabel, live_no_room: bool) -> Step {
    if live_no_room {
        Step::AcceptUnconfirmed(label)
    } else {
        Step::Reject(Why::Unknown(label))
    }
}

#[cfg(test)]
#[path = "tests_issue_2910_unknown.rs"]
mod tests_issue_2910_unknown;
