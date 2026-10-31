//! Candidate selection: a pure function of the profiles, their readings, the
//! threshold, the exclusions, the overflow profiles and an eligibility filter
//! supplied by the caller.
//!
//! Overflow profiles (issue #1606) are chosen only by [`select_with_overflow`]:
//! after every plan profile, and only when the shared overflow [`Gate`]
//! allows it. The gate is the running rotator's, thresholds included, so
//! both measure plan capacity the same way.

use super::usage::{Reading, Window};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::Gate;
use std::cmp::Ordering;
use std::collections::BTreeMap;

pub struct Pool<'a> {
    pub profiles: &'a [ProfileName],
    pub readings: &'a BTreeMap<String, Reading>,
    pub threshold: u8,
    pub exclude: &'a [ProfileName],
    pub now: i64,
}

/// A window's usage, or 0 once its reset time has passed.
pub fn adjusted(w: Option<&Window>, now: i64) -> f64 {
    match w {
        Some(w) if w.resets_at > now => w.used_percentage,
        _ => 0.0,
    }
}

fn worst(r: &Reading, now: i64) -> f64 {
    adjusted(r.five_hour.as_ref(), now).max(adjusted(r.seven_day.as_ref(), now))
}

/// Either adjusted window at or above the threshold. No reading: not exhausted.
pub fn is_exhausted(r: Option<&Reading>, threshold: u8, now: i64) -> bool {
    r.is_some_and(|r| worst(r, now) >= f64::from(threshold))
}

/// The eligible profile with the most quota left: known readings by their
/// worse window (lowest first), then profiles without a reading; ties by
/// name. `eligible` is asked only about profiles that pass every other rule.
pub fn select<'a>(
    pool: &Pool<'a>,
    current: &str,
    eligible: impl Fn(&ProfileName) -> bool,
) -> Option<&'a ProfileName> {
    select_by(
        pool,
        current,
        |r| !is_exhausted(r, pool.threshold, pool.now),
        eligible,
    )
}

/// [`select`] with `usable` in place of "under the threshold": a profile
/// whose reading (`None` when it has none) fails `usable` is skipped.
fn select_by<'a>(
    pool: &Pool<'a>,
    current: &str,
    usable: impl Fn(Option<&Reading>) -> bool,
    eligible: impl Fn(&ProfileName) -> bool,
) -> Option<&'a ProfileName> {
    let mut known: Vec<(f64, &'a ProfileName)> = Vec::new();
    let mut unknown: Vec<&'a ProfileName> = Vec::new();
    for p in pool.profiles {
        if p.as_str() == current || pool.exclude.contains(p) {
            continue;
        }
        let reading = pool.readings.get(p.as_str());
        if !usable(reading) || !eligible(p) {
            continue;
        }
        match reading {
            Some(r) => known.push((worst(r, pool.now), p)),
            None => unknown.push(p),
        }
    }
    known.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.as_str().cmp(b.1.as_str()))
    });
    unknown.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    known
        .first()
        .map(|k| k.1)
        .or_else(|| unknown.first().copied())
}

/// The overflow side of a selection (issue #1606).
pub struct OverflowPool<'a> {
    /// The overflow profiles in the order they are tried: tier, then file
    /// order within a tier.
    pub order: &'a [ProfileName],
    /// Overflow profiles with an active spend block.
    pub blocked: &'a [String],
    /// `None` when overflow may not be used at all: the gate's settings or
    /// the spend blocks could not be read.
    pub gate: Option<Gate>,
    /// The API-key profile (issue #1700), also last in `order`. Its block,
    /// when active, is in `blocked`.
    pub platform: Option<&'a ProfileName>,
    /// Every API-key profile, marked or not (issue #1804). None is a plan
    /// login: only `order` makes one a target.
    pub not_plan: &'a [ProfileName],
    /// Kinds could not be read, so any profile may bill per token: nothing
    /// is selected.
    pub kinds_unknown: bool,
}

impl OverflowPool<'_> {
    /// Whether `p` is a plan login: neither overflow nor an API-key profile
    /// (issue #1804). The one test every auto-switch decision uses, so an
    /// API-key profile's missing reading never counts as free capacity.
    pub fn is_plan(&self, p: &ProfileName) -> bool {
        !self.order.contains(p) && !self.not_plan.contains(p)
    }
}

/// A reading's adjusted (session, week) use: five-hour and seven-day, 0 for
/// a missing or reset window.
fn session_week(r: &Reading, now: i64) -> (f64, f64) {
    (
        adjusted(r.five_hour.as_ref(), now),
        adjusted(r.seven_day.as_ref(), now),
    )
}

/// Whether a plan reading has capacity by the gate's test, the one the
/// rotator applies ([`Gate::plan_capacity`]).
pub fn gate_capacity(r: &Reading, g: &Gate, now: i64) -> bool {
    let (session, week) = session_week(r, now);
    g.plan_capacity(session, week)
}

/// Whether a plan reading has room: both windows below 100%, the
/// rotator's test (`rotate::plan_room`, issue #1804). Only room keeps work
/// off the API key; capacity is the stricter test above.
pub fn has_room(r: &Reading, now: i64) -> bool {
    let (session, week) = session_week(r, now);
    crate::commands::profile::rotate::plan_room::room(session, week)
}

/// When a plan reading without capacity by the gate's test has it again:
/// the reset of the window at its threshold, or the later of the two when
/// both are, as the rotator computes it. `None` when neither is.
pub fn regains(r: &Reading, g: &Gate, now: i64) -> Option<i64> {
    regains_at(
        g,
        session_week(r, now),
        (
            r.five_hour.as_ref().map(|w| w.resets_at),
            r.seven_day.as_ref().map(|w| w.resets_at),
        ),
    )
}

/// [`regains`] from (session, week) use and their reset times (`None` when
/// unknown), for callers that hold the parts rather than a [`Reading`].
pub fn regains_at(
    g: &Gate,
    (session, week): (f64, f64),
    (session_reset, week_reset): (Option<i64>, Option<i64>),
) -> Option<i64> {
    let (session_hit, week_hit) = g.at_threshold(session, week);
    match (session_hit, week_hit) {
        (true, false) => session_reset,
        (false, true) => week_reset,
        (true, true) => Some(session_reset?.max(week_reset?)),
        (false, false) => None,
    }
}

/// Whether a session on an overflow profile may move back to a plan profile
/// with reading `r` (issue #1606). This is the one rule for the move back,
/// used by the selection ([`select_with_overflow`]) and by the status line's
/// cheap pre-check (`swap::overflow_precheck`):
///
/// - with a gate, the gate's capacity test alone ([`gate_capacity`], the
///   rotator's `--session-at` and `--week-at`), whatever the auto-switch
///   threshold, so a session leaves overflow exactly when the rotator would;
/// - without one (overflow closed), under the auto-switch threshold, as for
///   any other move.
///
/// A profile with no reading may always be moved back to.
pub fn may_return_to(r: Option<&Reading>, gate: Option<&Gate>, threshold: u8, now: i64) -> bool {
    match (r, gate) {
        (None, _) => true,
        (Some(r), Some(g)) => gate_capacity(r, g, now),
        (Some(r), None) => !is_exhausted(Some(r), threshold, now),
    }
}

/// Whether a session on an overflow profile leaves it for a plan profile
/// with reading `r`: [`may_return_to`] or, with a gate, room ([`has_room`],
/// issue #1804). The status line's cheap pre-check uses this, so it never
/// refuses a move the selection makes.
pub fn way_off(r: Option<&Reading>, gate: Option<&Gate>, threshold: u8, now: i64) -> bool {
    may_return_to(r, gate, threshold, now)
        || (gate.is_some() && r.is_some_and(|r| has_room(r, now)))
}

/// Whether overflow profile `name` may be used or stayed on: it has no
/// active spend block. Its own plan reading does not count (issue #1694:
/// "the plan limits shown for an overflow profile count as exhausted, but
/// that profile is still usable"); the rotator's `next_overflow` skips an
/// overflow profile only for a spend limit or a failed probe, and
/// auto-switch has no probe of its own, only the usage readings. The one
/// test for the selection ([`select_with_overflow`]), the capacity file
/// (`capacity::compute`) and the status line's pre-check
/// (`swap::overflow_precheck`).
pub fn overflow_usable(name: &str, blocked: &[String]) -> bool {
    !blocked.iter().any(|b| b == name)
}

/// When the first plan profile, `current` included unless it is an overflow
/// profile, regains capacity by `gate`'s test: what the wait is measured
/// against in [`select_with_overflow`], and what the per-token reason line
/// reports. `None` when no reading gives a reset.
pub fn earliest_plan_reset(
    pool: &Pool<'_>,
    ov: &OverflowPool<'_>,
    gate: &Gate,
    current: &str,
) -> Option<i64> {
    let on_overflow = ov.order.iter().any(|o| o.as_str() == current);
    pool.profiles
        .iter()
        .filter(|p| !pool.exclude.contains(p) && ov.is_plan(p))
        .filter_map(|p| pool.readings.get(p.as_str()))
        .chain((!on_overflow).then(|| pool.readings.get(current)).flatten())
        .filter_map(|r| regains(r, gate, pool.now))
        .min()
}

/// [`select`] with overflow profiles. With a gate, "plan capacity" is the
/// gate's test ([`Gate::plan_capacity`], the rotator's `--session-at` and
/// `--week-at`), so auto-switch and the rotator agree on when money is
/// spent. In order, the rotator's (`rotate::overflow`, issue #1804):
///
/// 1. The best plan profile (the overflow profiles excluded) that has
///    capacity by the gate's test, or has no reading. From a plan profile it
///    must also be under the auto-switch threshold, as for any plan-to-plan
///    move. From an overflow profile the gate's test alone decides
///    ([`may_return_to`]), even with an auto-switch threshold below
///    `--session-at` or `--week-at`.
/// 2. From an overflow profile, the plan profile with room ([`has_room`])
///    and the most quota left: it costs nothing.
/// 3. From the API-key profile (issue #1700): the first usable credit
///    profile, since credits cost less. `None` when `current` is an overflow
///    profile without a spend block: it stays there.
/// 4. While any plan profile, `current` included, has room, no per-token
///    profile at all: the best plan profile under the auto-switch threshold
///    ([`select`]), else the one with room that beats `current`, else `None`.
/// 5. The first overflow profile in `ov.order` that is not `current`, not
///    excluded, not blocked and `eligible`, when the gate's wait holds.
/// 6. Otherwise step 4's plan profile, which still beats an ALERT.
///
/// An overflow profile's own plan reading plays no part in steps 3 and 5
/// ([`overflow_usable`]): a credit account at its plan limit is the case
/// overflow exists for, and the rotator ignores that reading too.
///
/// Without a gate (overflow closed) this is [`select`] on the plan profiles.
/// With kinds unknown it is `None`. An API-key profile ([`OverflowPool::not_plan`])
/// is never one of the plan profiles, and a session on an unmarked one never
/// moves onto the API-key tier.
/// A plan profile with no reading never counts as capacity or room in steps
/// 4 and 5; if it is eligible, step 1 has already picked it.
pub fn select_with_overflow<'a>(
    pool: &Pool<'a>,
    ov: &OverflowPool<'a>,
    current: &str,
    eligible: impl Fn(&ProfileName) -> bool,
) -> Option<&'a ProfileName> {
    if ov.kinds_unknown {
        return None;
    }
    let mut plan_exclude = pool.exclude.to_vec();
    plan_exclude.extend(pool.profiles.iter().filter(|p| !ov.is_plan(p)).cloned());
    let plan_pool = Pool {
        profiles: pool.profiles,
        readings: pool.readings,
        threshold: pool.threshold,
        exclude: &plan_exclude,
        now: pool.now,
    };
    // `select` borrows `plan_exclude`; hand back the caller's own name.
    let own = |p: Option<&ProfileName>| {
        let p = p?.clone();
        pool.profiles.iter().find(|q| **q == p)
    };
    let fallback = || own(select(&plan_pool, current, &eligible));
    let Some(gate) = ov.gate else {
        return fallback();
    };
    let has_capacity = |p: &ProfileName| {
        pool.readings
            .get(p.as_str())
            .is_none_or(|r| gate_capacity(r, &gate, pool.now))
    };
    let on_overflow = ov.order.iter().any(|o| o.as_str() == current);
    let back = if on_overflow {
        select_by(
            &plan_pool,
            current,
            |r| may_return_to(r, Some(&gate), pool.threshold, pool.now),
            &eligible,
        )
    } else {
        select(&plan_pool, current, |p| has_capacity(p) && eligible(p))
    };
    if let Some(p) = own(back) {
        return Some(p);
    }
    let with_room = || {
        let roomy = |r: Option<&Reading>| r.is_some_and(|r| has_room(r, pool.now));
        own(select_by(&plan_pool, current, roomy, &eligible))
    };
    if on_overflow && let Some(p) = with_room() {
        return Some(p);
    }
    if on_overflow
        && ov.platform.is_some_and(|p| p.as_str() == current)
        && let Some(o) = ov.order.iter().find(|o| {
            let n = o.as_str();
            n != current
                && Some(*o) != ov.platform
                && pool.profiles.contains(o)
                && !pool.exclude.contains(o)
                && overflow_usable(n, ov.blocked)
                && eligible(o)
        })
    {
        return Some(o);
    }
    if on_overflow && overflow_usable(current, ov.blocked) {
        return None;
    }
    let plan: Vec<&ProfileName> = pool
        .profiles
        .iter()
        .filter(|p| !plan_exclude.contains(p))
        .collect();
    let current_reading = (!on_overflow).then(|| pool.readings.get(current)).flatten();
    let earliest = earliest_plan_reset(pool, ov, &gate, current);
    // Issue #1804: per-token profiles only when no plan profile, the current
    // one included, has room. A missing reading is no room.
    let plan_room = plan
        .iter()
        .filter_map(|p| pool.readings.get(p.as_str()))
        .chain(current_reading)
        .any(|r| has_room(r, pool.now));
    if plan_room {
        let beats_current =
            |p: &&ProfileName| match (current_reading, pool.readings.get(p.as_str())) {
                (Some(c), Some(r)) => worst(r, pool.now) < worst(c, pool.now),
                _ => true,
            };
        return fallback().or_else(|| with_room().filter(beats_current));
    }
    let on_key = ov.not_plan.iter().any(|k| k.as_str() == current);
    if gate.far(earliest, pool.now)
        && let Some(o) = ov.order.iter().find(|o| {
            let n = o.as_str();
            n != current
                && !(on_key && Some(*o) == ov.platform)
                && pool.profiles.contains(o)
                && !pool.exclude.contains(o)
                && overflow_usable(n, ov.blocked)
                && eligible(o)
        })
    {
        return Some(o);
    }
    fallback()
}
