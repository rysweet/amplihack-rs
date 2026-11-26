//! The pre-swap confirmation step (issue #2910, D6): for a stale target a
//! decision chose, whether it is read again, accepted without a fresh
//! reading, or rejected so that the decision runs again and reaches the
//! next-ranked login. A target that is not stale needs no step: its reading
//! stands. Pure: the freshness gate (`fresh::settle`) gathers the facts,
//! makes any read and writes the lines.
//!
//! A target is swapped to only on a fresh reading (issue #3105, which
//! withdraws #2910's D7): a target whose probe failed, or whose confirming
//! read fails, comes back unknown or is older than a reading already held,
//! is rejected. The one exception is issue #2820's: when the live login has
//! a standing recorded block ([`Facts::live_blocked`]: a spend, session,
//! week or credit limit, or the API key's block), staying on it is never
//! better than moving, so a target with a recent reading
//! ([`Facts::recent`]) is accepted on that reading without a fresh one. A
//! live login that is merely full, with no block, gets no such move: #3105
//! pins that stay, and issue #3130 holds the decision on widening it.
//!
//! The exception is not this module's invention. The 23 November report on
//! #2820 asks for it ("when the live login is spend-blocked, staying on it
//! is never better than moving to any measured login"), and point 3 of the
//! owner's brief for the fix restates it for a spend- or usage-blocked live
//! login. "Rotate confirmation" in docs/reference/profile-command.md quotes
//! it and sets out how it is narrower than the D7 rule #3105 withdrew.
//!
//! | The stale target | Step |
//! |------------------|------|
//! | its probe failed this cycle, live login blocked, its reading recent | [`Step::AcceptUnconfirmed`] |
//! | this cycle's re-reads are used up, live login blocked, its reading recent | [`Step::AcceptStale`] ([`Limit::Count`]) |
//! | this cycle's re-reads are used up | [`Step::NotTried`] ([`Limit::Count`]) |
//! | its probe failed this cycle | [`Step::Reject`] ([`Why::Unknown`], or [`Why::Unfit`] with no label) |
//! | the cycle can read no more, live login blocked, its reading recent | [`Step::AcceptStale`] ([`Limit::Time`]) |
//! | the cycle can read no more | [`Step::NotTried`] ([`Limit::Time`]) |
//! | it can be read | [`Step::ReadAgain`] |
//!
//! Which target is chosen is the decision's, unchanged (issue #3105): a
//! login with a standing block is never eligible, and a reading older than
//! one already held is never ranked, so neither can be accepted here. The
//! acceptance adds no read, cap, count or timer.
//!
//! A probe that failed this cycle is never read again in it: there is
//! nothing to read, only to decide. A rejection made without a read counts
//! against the cap of [`MAX_REREADS_PER_CYCLE`], so a cycle writes at most
//! three such `reject:` lines (E5). An acceptance without a fresh reading
//! makes no read and writes no `reject:` line, so the cap does not bound it.
//! No retry loop, backoff or timer is added: the gate's existing cap and
//! shared time budget bound the step.

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
    /// Issue #2820: the live login has a standing recorded block
    /// (`target::standing_block`, or the API key's block). Never true for an
    /// overflow live login (E5b), and never for one that is only full.
    pub live_blocked: bool,
    /// The label of the target's probe, when it was unknown.
    pub label: Option<ProbeLabel>,
    /// Issue #2820: the reading the decision used for the target has a time
    /// (`fresh_reread::taken_at`), at most `LAST_GOOD_MAX_AGE_SECS` old, and
    /// later than when the target was last left or damped
    /// (`fresh_reread::recent`).
    pub recent: bool,
}

impl Facts {
    /// Issue #2820: the target may be taken on the reading the decision
    /// used, without a fresh one: the live login is blocked and that reading
    /// is recent.
    fn may_take_unread(&self) -> bool {
        self.live_blocked && self.recent
    }
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
    /// A model's week is at or above the swap threshold.
    ModelWeek(u8),
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
    /// Issue #2820: its probe failed while the live login is blocked, and
    /// the reading the decision used (its last good one) is recent: swap on
    /// that reading, without a fresh one, and say so on the swap line.
    AcceptUnconfirmed(ProbeLabel),
    /// Issue #2820: the gate stopped short of reading the target again (the
    /// limit) while the live login is blocked, and its reading is recent:
    /// swap on that reading, without a fresh one, and say so.
    AcceptStale(Limit),
    /// Read it again and decide on the new reading.
    ReadAgain,
    /// Reject it and decide again without it.
    Reject(Why),
    /// Make it unknown for this cycle without a read.
    NotTried(Limit),
}

/// Why a swap target was accepted without a fresh reading. In memory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unconfirmed {
    /// Its reading is unknown, with the probe's label.
    Unknown(ProbeLabel),
    /// The gate stopped short of reading it (`limit`), and the reading the
    /// decision used is `age` seconds old.
    NotTried { limit: Limit, age: u64 },
}

impl From<ProbeLabel> for Unconfirmed {
    fn from(label: ProbeLabel) -> Self {
        Self::Unknown(label)
    }
}

impl Unconfirmed {
    /// The probe's label, for an unknown reading only.
    pub fn label(&self) -> Option<ProbeLabel> {
        match self {
            Self::Unknown(label) => Some(*label),
            Self::NotTried { .. } => None,
        }
    }

    /// The text in ` unconfirmed (<text>)` on the swap line.
    pub fn text(&self) -> String {
        match self {
            Self::Unknown(label) => label.as_str().to_string(),
            Self::NotTried {
                limit: Limit::Count,
                ..
            } => format!("re-read limit {MAX_REREADS_PER_CYCLE}"),
            Self::NotTried {
                limit: Limit::Time, ..
            } => "time limit".to_string(),
        }
    }
}

/// The step for one stale chosen target (D6, issue #3105, issue #2820).
pub fn next(f: &Facts) -> Step {
    // Issue #2820, before the cap: this acceptance reads nothing, so there
    // is nothing for the cap to bound.
    if f.failed_now
        && f.may_take_unread()
        && let Some(label) = f.label
    {
        return Step::AcceptUnconfirmed(label);
    }
    if f.attempts >= MAX_REREADS_PER_CYCLE {
        return stopped_short(f, Limit::Count);
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
        return stopped_short(f, Limit::Time);
    }
    Step::ReadAgain
}

/// The step when the gate stops short of a read at `limit` (issue #2820):
/// accepted on its reading when the live login is blocked and the reading
/// is recent, else unknown for the cycle as before.
fn stopped_short(f: &Facts, limit: Limit) -> Step {
    if f.may_take_unread() {
        Step::AcceptStale(limit)
    } else {
        Step::NotTried(limit)
    }
}

/// The step for a target whose confirming re-read came back unknown with
/// `label`: accepted on the reading the decision used when the live login
/// is blocked and that reading is recent (issue #2820), else rejected
/// (issue #3105). A re-read older than one already held is always rejected
/// by the gate before this is asked.
pub fn after_unknown(label: ProbeLabel, f: &Facts) -> Step {
    if f.may_take_unread() {
        Step::AcceptUnconfirmed(label)
    } else {
        Step::Reject(Why::Unknown(label))
    }
}

#[cfg(test)]
#[path = "tests_issue_2910_unknown.rs"]
mod tests_issue_2910_unknown;
