//! Builds the policy's inputs from the profile view and the governor's own
//! history: budgets, the samples behind the burn rate, each window's rates
//! and the rate signal (D4, D12).

use super::budget::budget_from_snapshot;
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

/// The budget of every profile with a usable weekly reading.
pub fn budgets(snaps: &[ProfileSnapshot], now: i64) -> Vec<Budget> {
    snaps
        .iter()
        .filter_map(budget_from_snapshot)
        .map(|mut b| {
            b.weekly = rolled(b.weekly, now);
            b.session = b.session.map(|s| rolled(s, now));
            b
        })
        .collect()
}

fn sample(w: Option<(f64, i64)>, observed_at: i64) -> Option<Sample> {
    let (used_pct, resets_at) = w?;
    (observed_at > 0 && used_pct.is_finite() && (0.0..=100.0).contains(&used_pct)).then_some(
        Sample {
            observed_at,
            used_pct,
            resets_at: Some(resets_at),
        },
    )
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
