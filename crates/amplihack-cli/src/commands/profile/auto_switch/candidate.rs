//! Candidate selection: a pure function of the profiles, their readings, the
//! threshold, the exclusions and an eligibility filter supplied by the caller.

use super::usage::{Reading, Window};
use crate::commands::profile::name::ProfileName;
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
