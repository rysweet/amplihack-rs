//! Issue #1907: what a stay or an ALERT records as capacity for the
//! limit-resume hook. Capacity is a fresh, successful reading of the live
//! login below 100% on both windows (or a swap or switch onto a login),
//! recorded at the time that reading was taken, on a login with no active
//! spend block. A pure rule: no file, process or clock. See "Rotate stays
//! and capacity" in docs/reference/profile-command.md.

use super::alert::{NO_CAPACITY, alert_once, clear_alert, should_alert_advisory};
use super::cycle::CycleOutcome;
use super::decide::{Probed, Reading};
use super::fresh;
use super::ledger::RotateLedger;
use super::log::Sink;
use super::observe::Probes;
use super::overflow::Tag;
use crate::commands::profile::name::ProfileName;

/// How much this cycle knows about the live login's room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    /// Read in this cycle, measured, below 100% on both windows.
    Fresh,
    /// Billed outside the plan windows: an API-key profile is live (#1700),
    /// or a stay on an overflow profile (#1606) whose probe was read in this
    /// cycle. Counts for a stay, never to make an ALERT advisory.
    Platform,
    /// Anything else: a failed, carried, unknown, still-running or
    /// spend-limited probe, or a reading at or above 100%.
    Unread,
}

impl Room {
    /// The live login's room from its probe (`None` from a stub that kept
    /// none) and its reading. An allow-list: only what is shown to be fresh
    /// and below 100% is [`Room::Fresh`].
    pub fn of(probe: Option<&Probed>, reading: &Reading, platform_live: bool) -> Self {
        if platform_live {
            return Self::Platform;
        }
        let clean = probe.is_none_or(|p| trusted(p) && below_100(&p.reading));
        if clean && below_100(reading) {
            Self::Fresh
        } else {
            Self::Unread
        }
    }

    /// The cycle's room: [`Room::of`] the live probe, except that a stay on
    /// an overflow profile (`tag` `on`) whose probe was read in this cycle
    /// runs on credits, so its plan windows at 100% do not make it unread.
    /// A failed, carried, unknown or spend-limited overflow probe is still
    /// [`Room::Unread`].
    ///
    /// `spend_blocked`: the live login has an unexpired entry in the
    /// ledger's `overflow_blocked`, or this cycle blocks it. It cannot
    /// spend, whatever its plan windows read, so its room is
    /// [`Room::Unread`]: a stay on it is `stayed-unread`, and its ALERT is
    /// never advisory.
    pub fn live(
        probed: Option<&Probes>,
        reading: &Reading,
        platform: bool,
        tag: Option<Tag>,
        spend_blocked: bool,
    ) -> Self {
        if spend_blocked {
            return Self::Unread;
        }
        let probe = probed.map(|(p, _)| p);
        match Self::of(probe, reading, platform) {
            Self::Unread if tag == Some(Tag::On) && probe.is_some_and(read) => Self::Platform,
            room => room,
        }
    }

    /// Records a stay: `stayed`, clearing both alert fields, when the live
    /// login is known to have room; otherwise `stayed-unread`, leaving them
    /// as they are, since this cycle learnt nothing that would clear them.
    pub fn stay(self, l: &mut RotateLedger) -> CycleOutcome {
        match self {
            Self::Fresh | Self::Platform => {
                clear_alert(l);
                CycleOutcome::Stayed
            }
            Self::Unread => CycleOutcome::StayedUnread,
        }
    }

    /// Records an ALERT under `key`, logging `line` once per key. Only a
    /// no-capacity ALERT on a fresh live login below 100% is advisory: it is
    /// stored in `advisory_alert` and the cycle is a stay with capacity.
    /// Every other ALERT is stored in `alert` and blocks a resume.
    pub fn alert(
        self,
        out: &mut Sink,
        l: &mut RotateLedger,
        kind: &str,
        key: &str,
        t: i64,
        line: &str,
    ) -> CycleOutcome {
        if self == Self::Fresh && kind == NO_CAPACITY {
            if should_alert_advisory(l, key) {
                out.emit(t, line);
            } else {
                out.print(t, line);
            }
            return CycleOutcome::Stayed;
        }
        alert_once(out, l, key, t, line);
        CycleOutcome::Alerted
    }
}

/// When the capacity a [`CycleOutcome::Stayed`] cycle records was seen:
/// the time the live reading was taken ([`fresh::read_at`]), never later
/// than `measured_at`. A passive reading taken before a limit hit is no
/// evidence of room after it, so the cycle records the reading's own time
/// as `last_cycle.measured_at`, and the limit-resume hook, which wants a
/// time strictly after the limit, does not resume on it. `measured_at` when
/// there is no probe or no reading time (a stub, or the API key's
/// synthetic reading).
pub fn seen_at(probed: Option<&Probes>, measured_at: i64) -> i64 {
    probed
        .and_then(|(p, _)| fresh::read_at(p, measured_at))
        .map_or(measured_at, |at| at.min(measured_at))
}

/// Whether the live login `live` has an active spend block: an unexpired
/// entry in the ledger's `overflow_blocked`, or one this cycle adds
/// (`new_blocks`). Expiry matches `prune_blocks`: active while `now < until`.
pub fn spend_blocked(
    l: &RotateLedger,
    live: Option<&ProfileName>,
    new_blocks: &[ProfileName],
    now: i64,
) -> bool {
    live.is_some_and(|n| {
        new_blocks.contains(n)
            || l.overflow_blocked
                .get(n.as_str())
                .is_some_and(|b| now < b.until)
    })
}

/// The `last_cycle.measured_at` a cycle with `outcome` records: [`seen_at`]
/// for a [`CycleOutcome::Stayed`] cycle, so a passive reading taken before
/// a limit hit never resumes the session that limit stopped; `measured_at`
/// for every other outcome.
pub fn recorded_at(outcome: CycleOutcome, probed: Option<&Probes>, measured_at: i64) -> i64 {
    match outcome {
        CycleOutcome::Stayed => seen_at(probed, measured_at),
        _ => measured_at,
    }
}

/// Neither carried, unknown, still running nor spend-limited.
fn trusted(p: &Probed) -> bool {
    !(p.carried || p.unknown || p.still_running || p.spend_limit)
}

/// A probe that read the login in this cycle, whatever it found.
fn read(p: &Probed) -> bool {
    trusted(p) && !matches!(p.reading, Reading::Failed)
}

/// A measured reading strictly below 100 on both windows; NaN is not.
fn below_100(r: &Reading) -> bool {
    match *r {
        Reading::Measured { session, week } => session < 100.0 && week < 100.0,
        Reading::Exhausted { .. } | Reading::Failed => false,
    }
}

#[cfg(test)]
#[path = "tests_capacity.rs"]
mod tests_capacity;
#[cfg(test)]
#[path = "tests_issue_1907_cycle.rs"]
mod tests_issue_1907_cycle;
