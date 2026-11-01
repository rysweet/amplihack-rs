//! Issue #1797: the capacity summary of `profile usage` and the `capacity:`
//! line of `rotate status`, worked out from one snapshot of valid readings,
//! the weights, the usable threshold and earlier history entries. No I/O.
//!
//! - Headroom is summed in whole percentage points and is unweighted; the
//!   weighted figure is shown only when some current profile's weight is
//!   not 1.0.
//! - Unknown readings are never counted as 0% or 100%: they are left out of
//!   `N`, every sum and the rate, and only listed as unknown.
//! - Usable now: session below the threshold and week below 100%.
//! - Next session reset: session-limited logins with weekly capacity and a
//!   known reset, grouped within 60 s of the earliest.
//! - Rate: from the oldest entry no more than 6 h old, which must be at
//!   least 4.5 h old; a reset, seen as `w_reset` passing or a fall in `w`,
//!   adds the earlier `w` back. Runway is weighted headroom over the rate.
//! - Rounding is half away from zero and happens only when printed; a value
//!   that is not finite prints as `null`.

use super::capacity_weight::Weights;
use super::usage_cmd::{sanitize_display, utc_time as iso};
use super::usage_history::{Entry, Point, Snapshot};
use serde::{Serialize, Serializer};

/// How far back the rate looks.
pub(super) const RATE_WINDOW: i64 = 6 * 3600;
/// The youngest start the rate accepts.
const RATE_MIN_SPAN: i64 = 4 * 3600 + 30 * 60;
/// Session resets this close to the earliest are one reset.
const RESET_GROUP: i64 = 60;
/// Weekly resets up to this far ahead are listed.
const RESETS_AHEAD: i64 = 86_400;
const LABEL_WIDTH: usize = 21;

/// What the summary is worked out from.
pub(super) struct Inputs<'a> {
    pub(super) current: &'a Snapshot,
    pub(super) weights: &'a Weights,
    pub(super) threshold: u8,
    /// Earlier entries, in any order; those outside the rate window are
    /// ignored.
    pub(super) history: &'a [Entry],
    pub(super) now: i64,
}

/// One weekly reset in the next 24 hours.
#[derive(Debug, Clone, PartialEq)]
struct WeeklyReset {
    at: i64,
    profile: String,
    /// Weighted login-weeks the reset gives back.
    restores: f64,
}

/// The worked-out summary. Figures are kept unrounded; they are rounded
/// only by [`Summary::text_lines`], [`Summary::status_line`] and
/// serialisation.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Summary {
    valid: usize,
    /// Σ (100 − w) over valid readings, in percentage points.
    headroom_points: u32,
    /// Σ weight · (100 − w).
    weighted_points: f64,
    /// Σ weight over valid readings.
    weight_total: f64,
    show_weighted: bool,
    usable: usize,
    threshold: u8,
    unknown: Vec<String>,
    next_session_reset: Option<(i64, Vec<String>)>,
    /// Weighted login-weeks an hour; `None` without enough history.
    rate: Option<f64>,
    runway: Option<f64>,
    resets: Vec<WeeklyReset>,
}

/// `x` rounded half away from zero to `dp` decimals; `None` when `x` is not
/// finite.
pub(super) fn rounded(x: f64, dp: i32) -> Option<f64> {
    if !x.is_finite() {
        return None;
    }
    let f = 10f64.powi(dp);
    let v = (x * f).round() / f;
    v.is_finite().then_some(v)
}

/// `x` rounded and printed with `dp` decimals.
fn fixed(x: f64, dp: i32) -> String {
    let width = usize::try_from(dp).unwrap_or(0);
    match rounded(x, dp) {
        Some(v) => format!("{v:.width$}"),
        None => "unknown".to_string(),
    }
}

fn label(name: &str, value: &str) -> String {
    format!("{name:<LABEL_WIDTH$}{value}")
}

fn shown(name: &str) -> String {
    sanitize_display(name, false)
}

/// Whether a reset came between `a` and `b`, which was read at `b_at`.
fn reset_between(a: &Point, b: &Point, b_at: i64) -> bool {
    a.w_reset.is_some_and(|r| r <= b_at) || b.w < a.w
}

/// Points of the week used from `a` to `b`. After a reset, everything `b`
/// shows was used since: the earlier `a.w` is added back to the fall.
fn used_between(a: &Point, b: &Point, b_at: i64) -> i64 {
    if reset_between(a, b, b_at) {
        i64::from(b.w)
    } else {
        i64::from(b.w) - i64::from(a.w)
    }
}

/// Weighted login-weeks an hour over the rate window, counting only
/// profiles valid at both ends. `None` without a start at least 4.5 h old
/// or with no profile at both ends.
fn rate_of(i: &Inputs<'_>) -> Option<f64> {
    let from = i.now.saturating_sub(RATE_WINDOW);
    let mut window: Vec<&Entry> = i
        .history
        .iter()
        .filter(|e| e.at >= from && e.at < i.now)
        .collect();
    window.sort_by_key(|e| e.at);
    let (start, rest) = window.split_first()?;
    let span = i.now.saturating_sub(start.at);
    if span < RATE_MIN_SPAN {
        return None;
    }
    let mut total = 0.0;
    let mut counted = false;
    for (name, now_point) in &i.current.points {
        let Some(mut prev) = start.profiles.get(name) else {
            continue;
        };
        counted = true;
        let mut used = 0i64;
        for e in rest {
            if let Some(p) = e.profiles.get(name) {
                used += used_between(prev, p, e.at);
                prev = p;
            }
        }
        used += used_between(prev, now_point, i.now);
        total += i.weights.of(name) * used as f64;
    }
    counted.then(|| total / 100.0 / (span as f64 / 3600.0))
}

/// Works out the summary.
pub(super) fn compute(i: &Inputs<'_>) -> Summary {
    let points = &i.current.points;
    let mut headroom_points = 0u32;
    let mut weighted_points = 0.0;
    let mut weight_total = 0.0;
    let mut usable = 0;
    for (name, p) in points {
        let left = 100 - p.w.min(100);
        let weight = i.weights.of(name);
        headroom_points += u32::from(left);
        weighted_points += weight * f64::from(left);
        weight_total += weight;
        if p.s.is_some_and(|s| s < i.threshold) && p.w < 100 {
            usable += 1;
        }
    }

    let candidates: Vec<(i64, &String)> = points
        .iter()
        .filter(|(_, p)| p.s.is_some_and(|s| s >= 100) && p.w < 100)
        .filter_map(|(n, p)| p.s_reset.map(|r| (r, n)))
        .collect();
    let next_session_reset = candidates.iter().map(|(r, _)| *r).min().map(|first| {
        let mut names: Vec<String> = candidates
            .iter()
            .filter(|(r, _)| *r <= first.saturating_add(RESET_GROUP))
            .map(|(_, n)| (*n).clone())
            .collect();
        names.sort();
        (first, names)
    });

    let until = i.now.saturating_add(RESETS_AHEAD);
    let mut resets: Vec<WeeklyReset> = points
        .iter()
        .filter_map(|(n, p)| {
            let at = p.w_reset.filter(|r| *r > i.now && *r <= until)?;
            let restores = i.weights.of(n) * f64::from(p.w) / 100.0;
            (restores > 0.0).then(|| WeeklyReset {
                at,
                profile: n.clone(),
                restores,
            })
        })
        .collect();
    resets.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.profile.cmp(&b.profile)));

    let rate = rate_of(i);
    let runway = rate
        .filter(|r| *r > 0.0)
        .map(|r| weighted_points / 100.0 / r);

    let mut unknown = i.current.unknown.clone();
    unknown.sort();
    Summary {
        valid: points.len(),
        headroom_points,
        weighted_points,
        weight_total,
        show_weighted: i
            .weights
            .any_differs_among(i.current.points.keys().map(String::as_str)),
        usable,
        threshold: i.threshold,
        unknown,
        next_session_reset,
        rate,
        runway,
        resets,
    }
}

impl Summary {
    fn login_weeks(&self) -> f64 {
        f64::from(self.headroom_points) / 100.0
    }

    fn headroom_pct(&self) -> Option<f64> {
        (self.valid > 0).then(|| f64::from(self.headroom_points) / self.valid as f64)
    }

    fn weighted_login_weeks(&self) -> f64 {
        self.weighted_points / 100.0
    }

    /// `X login-weeks of N (P%)` with the weighted figure when shown, or
    /// `unknown (no valid readings)`.
    fn headroom_text(&self) -> String {
        let Some(pct) = self.headroom_pct() else {
            return "unknown (no valid readings)".to_string();
        };
        let mut s = format!(
            "{} login-weeks of {} ({}%)",
            fixed(self.login_weeks(), 2),
            self.valid,
            fixed(pct, 0)
        );
        if self.show_weighted {
            s.push_str(&format!(
                "   [weighted: {} of {}]",
                fixed(self.weighted_login_weeks(), 2),
                fixed(self.weight_total, 2)
            ));
        }
        s
    }

    fn usable_text(&self) -> String {
        format!("{} of {} logins", self.usable, self.valid)
    }

    /// `~H h`, or `None` when the runway is unknown.
    fn runway_hours(&self) -> Option<String> {
        self.runway.map(|h| format!("~{} h", fixed(h, 0)))
    }

    /// The six lines printed after the table, labels padded to 21
    /// characters.
    pub(super) fn text_lines(&self) -> Vec<String> {
        let session = match &self.next_session_reset {
            None => "none".to_string(),
            Some((at, names)) => {
                let names: Vec<String> = names.iter().map(|n| shown(n)).collect();
                format!(
                    "{}  ({}: +{} usable)",
                    iso(*at),
                    names.join(", "),
                    names.len()
                )
            }
        };
        let runway = match (self.runway_hours(), self.rate) {
            (Some(h), Some(rate)) => {
                format!(
                    "{h} at the last 6 h rate ({} login-weeks/h)",
                    fixed(rate, 3)
                )
            }
            _ => "unknown".to_string(),
        };
        let resets = if self.resets.is_empty() {
            "none".to_string()
        } else {
            self.resets
                .iter()
                .map(|r| {
                    format!(
                        "{} {} +{}",
                        iso(r.at),
                        shown(&r.profile),
                        fixed(r.restores, 2)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        };
        let m = self.unknown.len();
        vec![
            label("weekly headroom", &self.headroom_text()),
            label(
                "usable now",
                &format!(
                    "{} (session < {}% and weekly < 100%)",
                    self.usable_text(),
                    self.threshold
                ),
            ),
            label("next session reset", &session),
            label("runway", &runway),
            label("resets in next 24 h", &resets),
            label(
                "unknown",
                &format!(
                    "{m} profile{} (readings timed out or failed; excluded from all figures)",
                    if m == 1 { "" } else { "s" }
                ),
            ),
        ]
    }

    /// The `rotate status` line for a summary of the entry taken at `at`,
    /// with its age at `now`.
    pub(super) fn status_line(&self, at: i64, now: i64) -> String {
        let age = now.saturating_sub(at).max(0);
        let age = match age {
            a if a < 60 => format!("{a}s"),
            a if a < 3600 => format!("{}m", a / 60),
            a if a < 86_400 => format!("{}h", a / 3600),
            a => format!("{}d", a / 86_400),
        };
        format!(
            "capacity: headroom {}, usable {}, runway {} (as of {}, {age} ago)",
            self.headroom_text(),
            self.usable_text(),
            self.runway_hours().unwrap_or_else(|| "unknown".to_string()),
            iso(at)
        )
    }

    fn json(&self) -> SummaryJson<'_> {
        SummaryJson {
            weekly_headroom_login_weeks: rounded(self.login_weeks(), 2),
            weekly_headroom_pct: self.headroom_pct().and_then(|p| rounded(p, 1)),
            weighted_headroom_login_weeks: rounded(self.weighted_login_weeks(), 2),
            usable_now: self.usable,
            valid_readings: self.valid,
            unknown: &self.unknown,
            next_session_reset: self.next_session_reset.as_ref().map(|(at, profiles)| {
                SessionResetJson {
                    at: iso(*at),
                    profiles,
                }
            }),
            rate_login_weeks_per_hour: self.rate.and_then(|r| rounded(r, 3)),
            runway_hours: self.runway.and_then(|h| rounded(h, 1)),
            weekly_resets_next_24h: self
                .resets
                .iter()
                .map(|r| WeeklyResetJson {
                    at: iso(r.at),
                    profile: &r.profile,
                    restores: rounded(r.restores, 2),
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct SessionResetJson<'a> {
    at: String,
    profiles: &'a [String],
}

#[derive(Serialize)]
struct WeeklyResetJson<'a> {
    at: String,
    profile: &'a str,
    restores: Option<f64>,
}

/// The documented `summary` object.
#[derive(Serialize)]
struct SummaryJson<'a> {
    weekly_headroom_login_weeks: Option<f64>,
    weekly_headroom_pct: Option<f64>,
    weighted_headroom_login_weeks: Option<f64>,
    usable_now: usize,
    valid_readings: usize,
    unknown: &'a [String],
    next_session_reset: Option<SessionResetJson<'a>>,
    rate_login_weeks_per_hour: Option<f64>,
    runway_hours: Option<f64>,
    weekly_resets_next_24h: Vec<WeeklyResetJson<'a>>,
}

impl Serialize for Summary {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.json().serialize(s)
    }
}
