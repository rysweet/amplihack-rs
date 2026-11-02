//! Builds the policy's inputs from the profile view and the governor's own
//! history: budgets, the samples behind the burn rate, each window's rates
//! and the rate signal (D4, D12).

use super::budget::{budget_from_snapshot, stale_reading};
use super::files::{History, push_sample};
use super::model::{Budget, Sample, WindowBudget};
use super::rate::{RateSignal, WindowRate, burn_rate, forecast, signal, sustainable};
use crate::commands::profile::governor_view::ProfileSnapshot;
use std::collections::BTreeMap;
use std::path::Path;

/// One profile's rates. `session` is `None` without a session window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProfileRates {
    pub session: Option<WindowRate>,
    pub weekly: WindowRate,
}

/// The profiles under `root`, read only.
pub fn snapshots(root: &Path) -> Vec<ProfileSnapshot> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .filter(|h| h.is_absolute());
    crate::commands::profile::governor_view::snapshot(root, home.as_deref())
}

/// A window whose reset has passed has started again from zero.
fn rolled(w: WindowBudget, now: i64) -> WindowBudget {
    match w.resets_at {
        Some(r) if r <= now => WindowBudget {
            used_pct: 0.0,
            resets_at: None,
        },
        _ => w,
    }
}

/// A window at or above 100% whose reset is still ahead. Run after
/// [`rolled`], so a window whose reset has passed is never exhausted.
fn exhausted(b: &Budget) -> bool {
    b.weekly.used_pct >= 100.0 || b.session.is_some_and(|s| s.used_pct >= 100.0)
}

/// The budget of every profile with a usable reading, each window rolled
/// over once its reset has passed.
///
/// A reading taken more than `max_age` seconds before `now`
/// (`governor.max_reading_age_secs`), or dated after it, is stale. Use in a
/// window only rises until its reset, so a stale reading is a lower bound on
/// use: it can show that a window is exhausted, never that it has room.
/// - A stale reading with a window exhausted and its reset still ahead is
///   kept. The profile then counts as exhausted at steps 2 and 3 and adds no
///   room, so a hold on it lasts until that reset passes or a fresh reading
///   arrives. Holding starts stops the readings being refreshed, so dropping
///   such a profile would empty the pool after `max_age` and step 1 would
///   admit everything (`no-readings`).
/// - Any other stale reading leaves its profile unknown, as an invalid one
///   does: an old 10% with its reset ahead would otherwise count as 90
///   points of room that later use may already have spent.
pub fn budgets(snaps: &[ProfileSnapshot], now: i64, max_age: u64) -> Vec<Budget> {
    snaps
        .iter()
        .filter_map(|s| {
            let mut b = budget_from_snapshot(s)?;
            b.weekly = rolled(b.weekly, now);
            b.session = b.session.map(|w| rolled(w, now));
            (!stale_reading(s, max_age, now) || exhausted(&b)).then_some(b)
        })
        .collect()
}

/// A history sample for one window, by the same rule as the budget
/// ([`super::budget::window`]); `None` without a time of observation.
fn sample(w: Option<(f64, i64)>, observed_at: i64) -> Option<Sample> {
    if observed_at <= 0 {
        return None;
    }
    let b = super::budget::window(w, observed_at)?;
    Some(Sample {
        observed_at,
        used_pct: b.used_pct,
        resets_at: b.resets_at,
    })
}

/// Adds each profile's latest reading to its rings and drops the rings of
/// profiles that are no longer listed.
pub fn record(history: &mut History, snaps: &[ProfileSnapshot]) {
    for s in snaps {
        if let Some(x) = sample(s.seven_day, s.observed_at) {
            push_sample(history.weekly.entry(s.name.clone()).or_default(), x);
        }
        if let Some(x) = sample(s.five_hour, s.observed_at) {
            push_sample(history.session.entry(s.name.clone()).or_default(), x);
        }
    }
    let listed = |name: &String| snaps.iter().any(|s| &s.name == name);
    history.weekly.retain(|name, _| listed(name));
    history.session.retain(|name, _| listed(name));
}

fn window_rate(w: &WindowBudget, ring: Option<&Vec<Sample>>, now: i64) -> WindowRate {
    let remaining = (100.0 - w.used_pct).clamp(0.0, 100.0);
    let burn = ring.and_then(|r| burn_rate(r));
    WindowRate {
        burn,
        sustainable: sustainable(remaining, w.resets_at, now),
        forecast: forecast(remaining, burn, now),
        resets_at: w.resets_at,
    }
}

/// Each profile's rates, and the signal from the active profile's windows
/// (`Unknown` when the active profile is not eligible or has no reading).
pub fn rates(
    budgets: &[Budget],
    history: &History,
    now: i64,
    band_pct: u32,
) -> (BTreeMap<String, ProfileRates>, RateSignal) {
    let mut out = BTreeMap::new();
    let mut windows = Vec::new();
    for b in budgets {
        let r = ProfileRates {
            weekly: window_rate(&b.weekly, history.weekly.get(&b.name), now),
            session: b
                .session
                .as_ref()
                .map(|s| window_rate(s, history.session.get(&b.name), now)),
        };
        // Feedback follows the active profile, where new work runs.
        if b.active && b.eligible {
            windows.push(r.weekly);
            windows.extend(r.session);
        }
        out.insert(b.name.clone(), r);
    }
    (out, signal(&windows, band_pct))
}
