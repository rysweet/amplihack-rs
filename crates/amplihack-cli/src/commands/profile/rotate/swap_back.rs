//! Issue #1870: swap-back damping. The ledger's `swapped_away` maps each
//! login the rotator left while it was full to the epoch seconds of that
//! swap. The freshness gate (`fresh`) counts a damped login's reading as
//! stale unless it was taken strictly after that time. See "Swap-back
//! damping" in docs/reference/profile-command.md.
//!
//! The map holds profile names and integers only, never an account or a
//! token.

use super::decide::Reading;
use super::fresh::{self, MAX_READING_AGE_SECS};
use super::fresh_msg;
use super::ledger::RotateLedger;
use super::observe::Probes;
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;

/// The most entries the map keeps; the oldest goes first.
pub const MAX_ENTRIES: usize = 16;
/// An entry older than this, the weekly window, is dropped.
pub const MAX_AGE_SECS: i64 = 7 * 86_400;

/// The entries a loaded ledger may keep: valid profile names with a time
/// above 0 and at most [`MAX_READING_AGE_SECS`] ahead of `now`, the newest
/// [`MAX_ENTRIES`]. A bad entry is dropped rather than making the whole
/// ledger `RotateInconsistent`, so it can never stop rotation.
pub fn valid(map: BTreeMap<String, i64>, now: i64) -> BTreeMap<String, i64> {
    let latest = now.saturating_add(MAX_READING_AGE_SECS);
    let mut kept: BTreeMap<String, i64> = map
        .into_iter()
        .filter(|(name, at)| ProfileName::parse(name).is_ok() && *at > 0 && *at <= latest)
        .collect();
    cap(&mut kept);
    kept
}

/// Drops entries more than [`MAX_AGE_SECS`] old, then the oldest beyond
/// [`MAX_ENTRIES`].
pub fn prune(map: &mut BTreeMap<String, i64>, now: i64) {
    map.retain(|_, at| now.saturating_sub(*at) <= MAX_AGE_SECS);
    cap(map);
}

/// Keeps the newest [`MAX_ENTRIES`], dropping the oldest first (by time, then
/// name). One sort, so a hand-edited ledger with many entries costs
/// O(n log n) on load rather than a scan per dropped entry.
fn cap(map: &mut BTreeMap<String, i64>) {
    let excess = map.len().saturating_sub(MAX_ENTRIES);
    if excess == 0 {
        return;
    }
    let mut by_age: Vec<(i64, String)> = map.iter().map(|(n, at)| (*at, n.clone())).collect();
    by_age.sort_unstable();
    for (_, name) in by_age.into_iter().take(excess) {
        map.remove(&name);
    }
}

/// Exhausted, or at or over `--session-at` or `--week-at`. A failed reading
/// is unknown, never full.
pub fn full(r: &Reading, s: &Settings) -> bool {
    match r {
        Reading::Exhausted { .. } => true,
        Reading::Measured { session, week } => {
            *session >= f64::from(s.session_at) || *week >= f64::from(s.week_at)
        }
        Reading::Failed => false,
    }
}

/// Measured below both `--session-at` and `--week-at`.
pub fn room(r: &Reading, s: &Settings) -> bool {
    matches!(r, Reading::Measured { .. }) && !full(r, s)
}

/// The record step's update of `l.swapped_away`, after the cycle's swap,
/// stay or ALERT. In order:
/// 1. lift each entry whose login has a reading in `probes` taken strictly
///    after the entry that shows room;
/// 2. record `left`, the login a committed swap moved off, at `now` when
///    its reading at decision time was full;
/// 3. prune by age and count.
///
/// Returns the `damp:` lines for the entries lifted. `measured_at` is when
/// the cycle started measuring, the reading time of a probe read in it.
pub fn update(
    l: &mut RotateLedger,
    s: &Settings,
    live: Option<&ProfileName>,
    probes: Option<&Probes>,
    left: Option<(&ProfileName, &Reading)>,
    measured_at: i64,
    now: i64,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some((live_probe, others)) = probes {
        let named = live.map(|n| (n, live_probe));
        for (name, p) in named.into_iter().chain(others.iter().map(|(n, p)| (n, p))) {
            let Some(at) = l.swapped_away.get(name.as_str()).copied() else {
                continue;
            };
            let later = fresh::read_at(p, measured_at).is_some_and(|r| r > at);
            if later && room(&p.reading, s) {
                l.swapped_away.remove(name.as_str());
                lines.push(fresh_msg::lifted(name));
            }
        }
    }
    if let Some((from, reading)) = left
        && full(reading, s)
    {
        l.swapped_away.insert(from.to_string(), now);
    }
    prune(&mut l.swapped_away, now);
    lines
}
