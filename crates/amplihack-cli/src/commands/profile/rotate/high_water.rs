//! Issue #3181: the highest reading the rotator has taken of each login, for
//! the session and for the week, in the window it was read in.
//!
//! Within one window a login's use never falls, so a lower reading in the
//! same window, before that window resets, is stale by construction. The
//! cycle's decision, swap-back damping and `profile usage` see a login at no
//! less than its highest figure. Kept in the rotate ledger as `high_water`;
//! see "Highest reading in a window" in docs/reference/profile-command.md.
//!
//! The map holds profile names, percentages and epoch seconds only, never an
//! account, address or token.

use super::super::decide::{Probed, Reading, Win};
use super::super::fresh::live::{Window, window};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::usage_parse::UsageReport;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// The most logins the map keeps; those whose windows reset last go first.
const MAX_ENTRIES: usize = 16;
/// The furthest ahead a kept reset may be: the weekly window and a day.
const MAX_AHEAD_SECS: i64 = 8 * 86_400;

/// One window's highest figure, and when that window resets.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Peak {
    pub used: f64,
    pub resets_at: i64,
}
// `used` is checked finite on load and on every raise, so equality is total.
impl Eq for Peak {}

/// A login's highest session and week figures.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peaks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Peak>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week: Option<Peak>,
}

impl Peaks {
    fn is_empty(&self) -> bool {
        self.session.is_none() && self.week.is_none()
    }
    /// The earliest reset of the two, for the cap.
    fn first_reset(&self) -> i64 {
        [self.session, self.week]
            .iter()
            .flatten()
            .map(|p| p.resets_at)
            .min()
            .unwrap_or(i64::MAX)
    }
}

/// The highest figures by profile name. A map that does not parse, or an
/// entry that does not, is dropped rather than failing the ledger's load.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct HighWater(BTreeMap<String, Peaks>);

impl<'de> Deserialize<'de> for HighWater {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let serde_json::Value::Object(map) = serde_json::Value::deserialize(d)? else {
            return Ok(Self::default());
        };
        let total = map.len();
        let kept: BTreeMap<String, Peaks> = map
            .into_iter()
            .filter_map(|(k, v)| serde_json::from_value::<Peaks>(v).ok().map(|p| (k, p)))
            .collect();
        let unparsed = total - kept.len();
        if unparsed > 0 {
            tracing::warn!(
                "rotate ledger: dropped {unparsed} high_water entries that did not parse"
            );
        }
        Ok(Self(kept))
    }
}

/// A stored peak that still applies at `now`: its window has not reset.
fn open(p: Option<Peak>, now: i64) -> Option<Peak> {
    p.filter(|p| p.resets_at > now)
}

/// Whether a stored peak bounds a reading whose window resets at `resets`:
/// the same window, an earlier one, or one whose reset is not known.
fn bounds(peak: Peak, resets: Option<i64>) -> bool {
    window(Some(peak.resets_at), resets) != Window::Later
}

/// The figure kept after a reading of `used` in the window resetting at
/// `resets`: `None` when the reading cannot be kept.
fn raised(stored: Option<Peak>, used: f64, resets: Option<i64>, now: i64) -> Option<Peak> {
    let r = resets?;
    if !used.is_finite() || r <= now || r > now.saturating_add(MAX_AHEAD_SECS) {
        return None;
    }
    let new = Peak {
        used: used.clamp(0.0, 100.0),
        resets_at: r,
    };
    match open(stored, now) {
        None => Some(new),
        Some(old) => match window(Some(old.resets_at), Some(r)) {
            Window::Later => Some(new),
            Window::Same if new.used > old.used => Some(new),
            _ => None,
        },
    }
}

/// `used`, raised to the peak that bounds it. Never NaN: an unreadable
/// figure counts as full.
fn floored(peak: Option<Peak>, used: f64, resets: Option<i64>, now: i64) -> f64 {
    let used = if used.is_nan() { 100.0 } else { used };
    match open(peak, now) {
        Some(p) if bounds(p, resets) => used.max(p.used),
        _ => used,
    }
}

/// The figures a probe is taken at: a measured reading's own, or an
/// exhausted one's windows. A failed probe has none.
fn figures(p: &Probed) -> Option<(f64, f64)> {
    let win = |w: Option<Win>| w.map_or(0.0, |w| w.used);
    match p.reading {
        Reading::Measured { session, week } => Some((session, week)),
        Reading::Exhausted { .. } => Some((win(p.windows.session), win(p.windows.week))),
        Reading::Failed => None,
    }
}

fn resets(w: Option<Win>) -> Option<i64> {
    w.and_then(|w| w.resets_at)
}

impl HighWater {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Keeps `p`'s figures for `name` where they are the highest in their
    /// window. A reading with no reset time, one whose window has already
    /// reset, and a non-finite figure change nothing; a reading whose session
    /// window has reset is old, and none of it is kept.
    pub fn raise(&mut self, name: &str, p: &Probed, now: i64) {
        let Some((session, week)) = figures(p) else {
            return;
        };
        // Read in a session window that has since reset: an old reading.
        if resets(p.windows.session).is_some_and(|r| r <= now) {
            return;
        }
        let old = self.0.get(name).cloned().unwrap_or_default();
        let s = raised(old.session, session, resets(p.windows.session), now);
        let w = raised(old.week, week, resets(p.windows.week), now);
        if s.is_none() && w.is_none() {
            return;
        }
        let entry = self.0.entry(name.to_string()).or_default();
        entry.session = s.or(entry.session);
        entry.week = w.or(entry.week);
    }

    /// `p` with a measured reading raised to `name`'s peaks where they bound
    /// it. Exhausted and failed readings are left as they were read.
    pub fn floor(&self, name: &str, p: &Probed, now: i64) -> Probed {
        let mut out = p.clone();
        if let Reading::Measured { session, week } = p.reading {
            let peaks = self.0.get(name).cloned().unwrap_or_default();
            out.reading = Reading::Measured {
                session: floored(peaks.session, session, resets(p.windows.session), now),
                week: floored(peaks.week, week, resets(p.windows.week), now),
            };
        }
        out
    }

    /// `profile usage`: each of `report`'s session and week figures at no
    /// less than `name`'s peak for its window. For display only; the caller
    /// keeps the report as read for the history and the records.
    pub(crate) fn floor_report(&self, name: &str, report: &mut UsageReport, now: i64) {
        let peaks = self.0.get(name).cloned().unwrap_or_default();
        for (w, peak) in [
            (&mut report.session, peaks.session),
            (&mut report.week_all, peaks.week),
        ] {
            if let (Some(w), Some(_)) = (w.as_mut(), peak) {
                w.used_percentage = floored(peak, w.used_percentage, w.resets_at, now);
            }
        }
    }

    /// Whether `p` reads below a peak of `name`'s that bounds it, so it is
    /// stale and cannot show room.
    pub fn below(&self, name: &str, p: &Probed, now: i64) -> bool {
        let Reading::Measured { session, week } = p.reading else {
            return false;
        };
        let Some(peaks) = self.0.get(name) else {
            return false;
        };
        let under = |peak: Option<Peak>, used: f64, r: Option<i64>| {
            open(peak, now).is_some_and(|k| bounds(k, r) && (used.is_nan() || used < k.used))
        };
        under(peaks.session, session, resets(p.windows.session))
            || under(peaks.week, week, resets(p.windows.week))
    }

    /// Drops the peaks whose windows have reset, then the entries left
    /// empty, then those beyond [`MAX_ENTRIES`].
    pub fn prune(&mut self, now: i64) {
        for peaks in self.0.values_mut() {
            peaks.session = open(peaks.session, now);
            peaks.week = open(peaks.week, now);
        }
        self.0.retain(|_, p| !p.is_empty());
        self.cap();
    }

    /// Keeps the [`MAX_ENTRIES`] whose windows reset first, by reset then
    /// name.
    fn cap(&mut self) {
        let excess = self.0.len().saturating_sub(MAX_ENTRIES);
        if excess == 0 {
            return;
        }
        let mut order: Vec<(i64, String)> = self
            .0
            .iter()
            .map(|(n, p)| (p.first_reset(), n.clone()))
            .collect();
        order.sort_unstable();
        for (_, name) in order.into_iter().rev().take(excess) {
            self.0.remove(&name);
        }
    }
}

/// How many entries, or peaks within them, a load dropped and why.
#[derive(Debug, Default, PartialEq, Eq)]
pub(in super::super) struct Dropped {
    pub bad_name: usize,
    pub bad_peak: usize,
    pub empty: usize,
    pub capped: usize,
}

impl Dropped {
    fn total(&self) -> usize {
        self.bad_name + self.bad_peak + self.empty + self.capped
    }
}

/// The entries a loaded ledger may keep: valid profile names with a figure
/// from 0 to 100 whose window resets after `now` and at most
/// [`MAX_AHEAD_SECS`] ahead, capped at [`MAX_ENTRIES`]. A bad entry is
/// dropped rather than making the ledger `RotateInconsistent`, and one
/// warning gives the counts by reason, never a name or a figure.
pub fn validate_on_load(h: HighWater, now: i64) -> HighWater {
    let (out, d) = validate(h, now);
    if d.total() > 0 {
        tracing::warn!(
            "rotate ledger: dropped high_water data on load: {} bad names, {} bad peaks, \
             {} empty entries, {} over the cap",
            d.bad_name,
            d.bad_peak,
            d.empty,
            d.capped
        );
    }
    out
}

/// [`validate_on_load`] without the warning, with the counts.
pub(in super::super) fn validate(h: HighWater, now: i64) -> (HighWater, Dropped) {
    let latest = now.saturating_add(MAX_AHEAD_SECS);
    let mut d = Dropped::default();
    let mut ok = |p: Option<Peak>| {
        let kept = p.filter(|p| {
            p.used.is_finite()
                && (0.0..=100.0).contains(&p.used)
                && p.resets_at > now
                && p.resets_at <= latest
        });
        d.bad_peak += usize::from(p.is_some() && kept.is_none());
        kept
    };
    let mut out = HighWater::default();
    for (name, p) in h.0 {
        if ProfileName::parse(&name).is_err() {
            d.bad_name += 1;
            continue;
        }
        let p = Peaks {
            session: ok(p.session),
            week: ok(p.week),
        };
        if p.is_empty() {
            d.empty += 1;
        } else {
            out.0.insert(name, p);
        }
    }
    let before = out.0.len();
    out.cap();
    d.capped = before - out.0.len();
    (out, d)
}

/// The load-time rule for the ledger's two maps of logins: swap-back
/// damping (#1870) and the highest readings. Bad entries are dropped.
pub(in super::super) fn on_load(l: &mut super::RotateLedger, now: i64) {
    l.swapped_away = super::super::swap_back::valid(std::mem::take(&mut l.swapped_away), now);
    l.high_water = validate_on_load(std::mem::take(&mut l.high_water), now);
}
