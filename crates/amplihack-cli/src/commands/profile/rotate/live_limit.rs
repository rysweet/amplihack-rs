//! Issue #1880: how this cycle's live probe counts. A pure classification of
//! the live login's probe, applied in `observe::decide_measured` before the
//! decision. Typed inputs only: no probe text is parsed, and an HTTP 429
//! from the usage request reads `Failed` like any other error, so it is
//! unknown, not a limit. See "Rotate failed live probes" in
//! docs/reference/profile-command.md.

use super::damp::FAILED_PROBE_LIMIT;
use super::decide::{Decision, Probed, Reading, Reason};

/// What this cycle's live probe means for the failed-probe count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOutcome {
    /// A `Measured` reading read in this cycle (or a passive reading).
    Fresh,
    /// A `Measured` reading from a read started in an earlier cycle (#1870).
    Carried,
    /// The live login has no room left: session or weekly exhausted
    /// (`spend_limit: false`), or a plan login at its spend limit
    /// (`spend_limit: true`).
    Limit { spend_limit: bool },
    /// Timed out, still running, could not run, error, unparsed or HTTP 429.
    Unknown,
    /// A spend limit on a live overflow profile: its block handles it.
    OverflowBlocked,
}

/// Classifies the live probe `p`; `live_overflow` is true when the live
/// profile is an overflow entry. A probe already reclassified by
/// [`for_decision`] (exhausted, with the spend-limit flag) classifies as
/// before, so a decision made again on it counts the same.
pub fn classify(p: &Probed, live_overflow: bool) -> LiveOutcome {
    match p.reading {
        Reading::Measured { .. } if p.carried => LiveOutcome::Carried,
        Reading::Measured { .. } => LiveOutcome::Fresh,
        Reading::Exhausted { .. } => LiveOutcome::Limit {
            spend_limit: p.spend_limit,
        },
        Reading::Failed if p.spend_limit && live_overflow => LiveOutcome::OverflowBlocked,
        Reading::Failed if p.spend_limit => LiveOutcome::Limit { spend_limit: true },
        Reading::Failed => LiveOutcome::Unknown,
    }
}

/// The probe the decision uses: a spend limit on a plan login reads as
/// `Reading::Exhausted`; every other probe is returned unchanged. The
/// spend-limit flag stays, so the log shows `spend limit`.
pub fn for_decision(p: &Probed, o: LiveOutcome) -> Probed {
    match (o, p.reading) {
        (LiveOutcome::Limit { spend_limit: true }, Reading::Failed) => Probed {
            reading: Reading::Exhausted {
                session: p.windows.session.map(|w| w.used),
                week: p.windows.week.map(|w| w.used),
            },
            ..p.clone()
        },
        _ => p.clone(),
    }
}

/// The count after this cycle: `Unknown` adds one (saturating); `Fresh` and
/// `Limit` clear it; `Carried` and `OverflowBlocked` leave it as it was.
pub fn next_count(prev: u32, o: LiveOutcome) -> u32 {
    match o {
        LiveOutcome::Unknown => prev.saturating_add(1),
        LiveOutcome::Fresh | LiveOutcome::Limit { .. } => 0,
        LiveOutcome::Carried | LiveOutcome::OverflowBlocked => prev,
    }
}

/// A threshold or plan-room swap away from a spend-limited plan login is
/// logged `reason=spend-limit`; anything else is returned unchanged.
pub fn relabel(d: Decision, o: LiveOutcome) -> Decision {
    match (d, o) {
        (
            Decision::Swap {
                to,
                reason: Reason::Threshold | Reason::PlanRoom,
            },
            LiveOutcome::Limit { spend_limit: true },
        ) => Decision::Swap {
            to,
            reason: Reason::SpendLimit,
        },
        (d, _) => d,
    }
}

/// A failed live probe below `FAILED_PROBE_LIMIT` keeps the plan-room swap
/// out of reach.
pub fn blocks_room(reading: &Reading, count: u32) -> bool {
    *reading == Reading::Failed && count < FAILED_PROBE_LIMIT
}
