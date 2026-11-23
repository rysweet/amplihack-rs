//! The pre-swap confirmation step (issue #2910, D6 and D7): for a stale
//! target a decision chose, whether it is read again, accepted without a
//! fresh reading, or rejected so that the decision runs again and reaches
//! the next-ranked login. A target that is not stale needs no step: its
//! reading stands. Pure: the freshness gate (`fresh::settle`) gathers the
//! facts, makes any read and writes the lines.
//!
//! | The stale target | Step |
//! |------------------|------|
//! | its probe failed this cycle, live login has no room now | [`Step::AcceptUnconfirmed`] |
//! | this cycle's re-reads are used up, live login has no room now, its reading is recent | [`Step::AcceptStale`] ([`Limit::Count`]) |
//! | this cycle's re-reads are used up | [`Step::NotTried`] ([`Limit::Count`]) |
//! | its probe failed this cycle, live login has room | [`Step::Reject`] ([`Why::Unknown`]) |
//! | the cycle can read no more, live login has no room now, its reading is recent | [`Step::AcceptStale`] ([`Limit::Time`]) |
//! | the cycle can read no more | [`Step::NotTried`] ([`Limit::Time`]) |
//! | it can be read | [`Step::ReadAgain`] |
//!
//! Issue #2820: when the live login has no room (a recorded block or a
//! window at 100%), staying on it is never better than moving, so a target
//! the cap or the time limit left unread is accepted on its reading
//! ([`Step::AcceptStale`]) when that reading is recent ([`Facts::recent`]).
//! Which target is chosen is the decision's, unchanged: a blocked login is
//! never eligible. The acceptance adds no read, cap, count or timer.
//!
//! A probe that failed this cycle is never read again in it: there is
//! nothing to read, only to decide. A rejection made without a read counts
//! against the cap of [`MAX_REREADS_PER_CYCLE`], so a cycle writes at most
//! three `reject:` lines (E5). An acceptance without a fresh reading makes no
//! read and writes no `reject:` line, so the cap does not bound it: D7 holds
//! after three re-reads too. No retry loop, backoff or timer is added: the
//! gate's existing cap and shared time budget bound the step.

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
    /// The live login has no room now (a window at 100%, or a block). Never
    /// true for an overflow live login (E5b).
    pub live_no_room: bool,
    /// The label of the target's probe, when it was unknown.
    pub label: Option<ProbeLabel>,
    /// Issue #2820: the reading the decision used for the target has a time
    /// (`fresh_reread::taken_at`), at most `LAST_GOOD_MAX_AGE_SECS` old, and
    /// later than when the target was last left or damped
    /// (`fresh_reread::recent`).
    pub recent: bool,
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
    /// Swap on the reading the decision used, without a fresh one, and say
    /// so on the swap line.
    AcceptUnconfirmed(ProbeLabel),
    /// Issue #2820: the gate stopped short of reading the target again (the
    /// limit) while the live login has no room, and its reading is recent:
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
    /// Its reading is unknown, with the probe's label (D7).
    Unknown(ProbeLabel),
    /// Issue #2820: the gate stopped short of reading it (`limit`), and the
    /// reading the decision used is `age` seconds old.
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

/// The step for one stale chosen target (D6, D7).
pub fn next(f: &Facts) -> Step {
    // D7 before the cap: this acceptance reads nothing, so there is nothing
    // for the cap to bound.
    if f.failed_now
        && f.live_no_room
        && let Some(label) = f.label
    {
        return Step::AcceptUnconfirmed(label);
    }
    if f.attempts >= MAX_REREADS_PER_CYCLE {
        return stopped_short(f, Limit::Count);
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
        return stopped_short(f, Limit::Time);
    }
    Step::ReadAgain
}

/// The step when the gate stops short of a read at `limit` (issue #2820):
/// accepted on its reading when the live login has no room and the reading
/// is recent, else unknown for the cycle as before.
fn stopped_short(f: &Facts, limit: Limit) -> Step {
    if f.live_no_room && f.recent {
        Step::AcceptStale(limit)
    } else {
        Step::NotTried(limit)
    }
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
