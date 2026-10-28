//! Candidate selection: a pure function of the profiles, their readings, the
//! threshold, the exclusions, the overflow profiles and an eligibility filter
//! supplied by the caller.
//!
//! Overflow profiles (issue #1606) are chosen only by [`select_with_overflow`]:
//! after every plan profile, and only when the shared overflow [`Gate`]
//! allows it. The rotator asks the same gate.

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
    let mut known: Vec<(f64, &'a ProfileName)> = Vec::new();
    let mut unknown: Vec<&'a ProfileName> = Vec::new();
    for p in pool.profiles {
        if p.as_str() == current || pool.exclude.contains(p) {
            continue;
        }
        let reading = pool.readings.get(p.as_str());
        if is_exhausted(reading, pool.threshold, pool.now) || !eligible(p) {
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
}

/// When an exhausted reading has capacity again: the latest reset of its
/// windows at or above `threshold`. `None` when no window is.
pub fn regains(r: &Reading, threshold: u8, now: i64) -> Option<i64> {
    [r.five_hour.as_ref(), r.seven_day.as_ref()]
        .into_iter()
        .flatten()
        .filter(|w| adjusted(Some(w), now) >= f64::from(threshold))
        .map(|w| w.resets_at)
        .max()
}

/// [`select`] with overflow profiles. In order:
///
/// 1. The best plan profile ([`select`] with the overflow profiles excluded).
///    This is also how a session on an overflow profile moves back: any plan
///    profile with capacity wins, whether or not the overflow profile is
///    exhausted.
/// 2. `None` when `current` is an overflow profile that is neither exhausted
///    nor blocked: it stays there until a plan profile has capacity.
/// 3. The first overflow profile in `ov.order` that is not `current`, not
///    excluded, not blocked, not exhausted and `eligible`, when the gate
///    allows it: no plan profile other than `current` has a reading under
///    the threshold, and the earliest plan reset is far enough away.
///
/// A plan profile with no reading never counts as capacity here; if it is
/// eligible, step 1 has already picked it.
pub fn select_with_overflow<'a>(
    pool: &Pool<'a>,
    ov: &OverflowPool<'a>,
    current: &str,
    eligible: impl Fn(&ProfileName) -> bool,
) -> Option<&'a ProfileName> {
    let is_overflow = |p: &ProfileName| ov.order.contains(p);
    let mut plan_exclude = pool.exclude.to_vec();
    plan_exclude.extend(ov.order.iter().cloned());
    let plan_pool = Pool {
        profiles: pool.profiles,
        readings: pool.readings,
        threshold: pool.threshold,
        exclude: &plan_exclude,
        now: pool.now,
    };
    // `select` borrows `plan_exclude`; hand back the caller's own name.
    if let Some(p) = select(&plan_pool, current, &eligible).cloned() {
        return pool.profiles.iter().find(|q| **q == p);
    }
    let exhausted = |name: &str| is_exhausted(pool.readings.get(name), pool.threshold, pool.now);
    let blocked = |name: &str| ov.blocked.iter().any(|b| b == name);
    let on_overflow = ov.order.iter().any(|o| o.as_str() == current);
    if on_overflow && !exhausted(current) && !blocked(current) {
        return None;
    }
    let gate = ov.gate?;
    let plan: Vec<&ProfileName> = pool
        .profiles
        .iter()
        .filter(|p| !is_overflow(p) && !pool.exclude.contains(p))
        .collect();
    let plan_has_capacity = plan.iter().any(|p| {
        p.as_str() != current
            && pool
                .readings
                .get(p.as_str())
                .is_some_and(|r| !is_exhausted(Some(r), pool.threshold, pool.now))
    });
    let current_reading = (!on_overflow).then(|| pool.readings.get(current)).flatten();
    let earliest = plan
        .iter()
        .filter_map(|p| pool.readings.get(p.as_str()))
        .chain(current_reading)
        .filter_map(|r| regains(r, pool.threshold, pool.now))
        .min();
    if !gate.allows(plan_has_capacity, earliest, pool.now) {
        return None;
    }
    ov.order.iter().find(|o| {
        let n = o.as_str();
        n != current
            && pool.profiles.contains(o)
            && !pool.exclude.contains(o)
            && !blocked(n)
            && !exhausted(n)
            && eligible(o)
    })
}
