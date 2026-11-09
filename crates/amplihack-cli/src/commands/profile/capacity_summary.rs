//! Issue #1797: the capacity summary of `profile usage` and the `capacity:`
//! line of `rotate status`, worked out from one snapshot of valid readings,
//! the weights, the usable threshold and earlier history entries. No I/O.
//!
//! - Headroom is summed in whole percentage points and is unweighted; the
//!   weighted figure is shown only when some current profile's weight is
//!   not 1.0.
//! - Unknown readings are never counted as 0% or 100%: they are left out of
//!   `N`, every sum and the rate, and only listed as unknown.
//! - Usable now: week below 100% and session below the threshold, or no
//!   session limit at all (issue #1818). A session not read is not usable.
//! - Next session reset: session-limited logins with weekly capacity and a
//!   known reset, grouped within 60 s of the earliest.
//! - Rate: from the oldest entry no more than 6 h old that covers every
//!   current login (a `profile usage` line, or a rotator line that read
//!   them all), which must be at least 4.5 h old. Issue #1970: each login
//!   counts its net rise in `w` within each week. A reset is `w_reset`
//!   passing or a fall of more than [`RESET_FALL`] points; a smaller fall is
//!   a stale reading and adds nothing, so the rate is never negative.
//!   Readings from a week already over, and a stale passive reading now,
//!   are left out. Each login's use is weighted. Runway is weighted
//!   headroom over the rate.
//! - `rotate status` takes no runway from a rotator line that left logins
//!   out; see [`status_line`].
//! - Rounding is half away from zero and happens only when printed; a value
//!   that is not finite prints as `null`.

use super::capacity_weight::Weights;
use super::usage_cmd::{sanitize_display, utc_time as iso};
use super::usage_history::{Entry, Point, Snapshot, Writer, stale};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;

/// How far back the rate looks.
pub(super) const RATE_WINDOW: i64 = 6 * 3600;
/// The youngest start the rate accepts.
const RATE_MIN_SPAN: i64 = 4 * 3600 + 30 * 60;
/// Issue #1970: a fall in a login's week of more than this many points
/// between two readings is a reset. A smaller fall is a stale reading.
pub(super) const RESET_FALL: u8 = 10;
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

/// Whether the week reset between reading `a` and reading `b`, which was
/// taken at `b_at`: `a`'s reset time has passed, or `w` fell by more than
/// [`RESET_FALL`] points. Anthropic can reset a week early, so a large fall
/// is a reset even while `a`'s reset time is still ahead.
fn reset_between(a: &Point, b: &Point, b_at: i64) -> bool {
    a.w_reset.is_some_and(|r| r <= b_at) || a.w.saturating_sub(b.w) > RESET_FALL
}

/// Whether `p`, taken at `at`, is a figure from a week that had already
/// ended when it was taken.
fn week_ended(p: &Point, at: i64) -> bool {
    p.w_reset.is_some_and(|r| r <= at)
}

/// Points of the week used over one login's `readings`, oldest first; the
/// first is the start.
///
/// The readings are split into weeks at each reset ([`reset_between`]).
/// Within a week the true figure only rises, so each week counts its
/// highest figure minus the figure it started at: the start's for the
/// first week, 0 for each later one. A lower reading within a week is a
/// stale one and adds nothing. The highest figure, not the last, is used,
/// because the last before a reset may be stale. Never negative: the
/// highest figure of a week is at least the figure it started at.
fn used_over(readings: &[(i64, &Point)]) -> i64 {
    let Some(((_, first), rest)) = readings.split_first() else {
        return 0;
    };
    let mut week_start = i64::from(first.w);
    let mut peak = week_start;
    let mut prev = *first;
    let mut used = 0i64;
    for (at, p) in rest {
        let w = i64::from(p.w);
        if reset_between(prev, p, *at) {
            used = used.saturating_add(peak - week_start);
            week_start = 0;
            peak = w;
        } else {
            peak = peak.max(w);
        }
        prev = *p;
    }
    used.saturating_add(peak - week_start)
}

/// Whether `e` may be the start of the rate for the logins in `current`.
///
/// A rotate cycle records only the logins it probed and never probes the
/// busy ones (a session running, leased, hosting or parked), which are the
/// logins being used up. A rotator line that leaves out a current login
/// would drop that login from the rate while the runway still divides the
/// headroom of every current login by it, so the runway would come out far
/// too long. Such a line is therefore never the start: the start is a
/// `profile usage` line (or an older line without a writer), which reads
/// every login, or a rotator line that read every current login. A login
/// that a `profile usage` start could not read is a failed reading and is
/// left out, as issue #1797 says. Rotator lines still count between the two
/// ends, where a missing login only skips that step.
fn can_start(e: &Entry, current: &Snapshot) -> bool {
    match e.by {
        Some(Writer::Rotate) => current.points.keys().all(|n| e.profiles.contains_key(n)),
        Some(Writer::Usage) | None => true,
    }
}

/// The parts of the rate, before the 4.5 h minimum is applied.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RateParts {
    /// The time of the start entry.
    pub(super) start: i64,
    /// Seconds from the start to now.
    pub(super) span: i64,
    /// For each login valid at both ends: the points of the week it used
    /// over the span, and its weight.
    pub(super) used: BTreeMap<String, (i64, f64)>,
}

/// The parts of the rate over the 6 h window. The start is the oldest
/// entry in the window that [`can_start`]; entries before it are not used.
/// `None` without such a start; the span is not checked here.
///
/// Each login with a reading at the start and now counts [`used_over`] its
/// readings: the start, each later entry that holds it, then now. A reading
/// from a week that had already ended when it was taken is left out, and a
/// login whose start reading is one is not counted. The current reading is
/// left out too when it is passive and [`stale`] against the login's last
/// reading in the window, since it is then a cached figure.
pub(super) fn rate_parts(i: &Inputs<'_>) -> Option<RateParts> {
    let from = i.now.saturating_sub(RATE_WINDOW);
    let mut window: Vec<&Entry> = i
        .history
        .iter()
        .filter(|e| e.at >= from && e.at < i.now)
        .collect();
    window.sort_by_key(|e| e.at);
    let first = window.iter().position(|e| can_start(e, i.current))?;
    let (start, rest) = window[first..].split_first()?;
    let mut used = BTreeMap::new();
    for (name, now_point) in &i.current.points {
        let Some(at_start) = start
            .profiles
            .get(name)
            .filter(|p| !week_ended(p, start.at))
        else {
            continue;
        };
        let mut readings = vec![(start.at, at_start)];
        readings.extend(rest.iter().filter_map(|e| {
            e.profiles
                .get(name)
                .filter(|p| !week_ended(p, e.at))
                .map(|p| (e.at, p))
        }));
        let last = readings.last().map(|(_, p)| *p);
        let current_stale =
            week_ended(now_point, i.now) || (now_point.passive && stale(last, now_point, i.now));
        if !current_stale {
            readings.push((i.now, now_point));
        }
        used.insert(name.clone(), (used_over(&readings), i.weights.of(name)));
    }
    Some(RateParts {
        start: start.at,
        span: i.now.saturating_sub(start.at),
        used,
    })
}

/// Weighted login-weeks an hour over the rate window: the [`rate_parts`]
/// with each login's points times its weight, over 100 and the span in
/// hours. `None` when the start is less than 4.5 h old or no login is valid
/// at both ends. Never negative.
fn rate_of(i: &Inputs<'_>) -> Option<f64> {
    let p = rate_parts(i)?;
    if p.span < RATE_MIN_SPAN || p.used.is_empty() {
        return None;
    }
    let total: f64 = p.used.values().map(|(u, w)| w * *u as f64).sum();
    Some(total / 100.0 / (p.span as f64 / 3600.0))
}

/// Usable now: the week is below 100% and the session is below the
/// threshold. A login with no session limit (issue #1818) has no session to
/// be below it and is usable on its week alone; a session that was not read
/// is not usable, because nothing shows it is below the threshold.
pub(super) fn usable_now(p: &Point, threshold: u8) -> bool {
    p.w < 100
        && match p.s {
            Some(s) => s < threshold,
            None => p.no_session_limit,
        }
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
        if usable_now(p, i.threshold) {
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

/// Whether `e` read every login it covers. A rotator line that lists a
/// login as unknown left it out: skipped as busy (and so likely being used
/// up) or a failed probe. A `profile usage` line, or an older line without
/// a writer, tried every login; its unknowns are failed readings.
///
/// A stale passive reading does not make its login unknown while the
/// history holds a figure of the week now running: the writer records that
/// figure instead (`usage_history::stale_replaced`, issue #1970), so a cycle
/// that read a cached figure still reads every login here.
fn reads_every_login(e: &Entry) -> bool {
    e.by != Some(Writer::Rotate) || e.unknown.is_empty()
}

/// The summary of entry `e`, with `history` (entries at or after `e.at` are
/// ignored by the rate) as the earlier entries.
fn summary_of(e: &Entry, history: &[Entry], weights: &Weights, threshold: u8) -> Summary {
    let current = Snapshot {
        at: e.at,
        points: e.profiles.clone(),
        unknown: e.unknown.clone(),
    };
    compute(&Inputs {
        current: &current,
        weights,
        threshold,
        history,
        now: e.at,
    })
}

/// The `capacity:` line of `rotate status`: `newest` is the newest history
/// entry, standing in for now, and `earlier` the entries before it, oldest
/// first; `now` gives the age.
///
/// Headroom and usable count are always those of `newest`. The runway is
/// too, when `newest` read every login. A rotator line that left logins out
/// holds only the idle ones, so its own runway would divide their headroom
/// by their near-zero use and come out far too long, or not at all, just
/// while the busy logins are being used up. Its runway is therefore taken
/// from the newest entry no more than six hours older that read every
/// login, and printed with that entry's time; with no such entry, or none
/// with a runway, it is `unknown`. Either way the line says how many logins
/// the newest cycle left out.
pub(super) fn status_line(
    newest: &Entry,
    earlier: &[Entry],
    weights: &Weights,
    threshold: u8,
    now: i64,
) -> String {
    let summary = summary_of(newest, earlier, weights, threshold);
    if reads_every_login(newest) {
        return summary.status_line(newest.at, now, newest.by);
    }
    let m = newest.unknown.len();
    let left_out = format!(
        "this cycle left out {m} login{}",
        if m == 1 { "" } else { "s" }
    );
    let from = newest.at.saturating_sub(RATE_WINDOW);
    let runway = earlier
        .iter()
        .rev()
        .filter(|e| e.at >= from && e.at < newest.at)
        .find(|e| reads_every_login(e))
        .and_then(|e| {
            let h = summary_of(e, earlier, weights, threshold).runway_hours()?;
            Some(format!("{h} from the {} entry ({left_out})", iso(e.at)))
        })
        .unwrap_or_else(|| format!("unknown ({left_out})"));
    summary.status_line_with_runway(newest.at, now, newest.by, &runway)
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
                    "{m} profile{} (no valid reading; excluded from all figures)",
                    if m == 1 { "" } else { "s" }
                ),
            ),
        ]
    }

    /// The `rotate status` line for a summary of the entry taken at `at`,
    /// with its age at `now`. `by` names the writer, because `N` differs:
    /// `profile usage` reads every subscription login, a rotate cycle only
    /// the profiles it probed. A line with no known writer names none.
    pub(super) fn status_line(&self, at: i64, now: i64, by: Option<Writer>) -> String {
        let runway = self.runway_hours().unwrap_or_else(|| "unknown".to_string());
        self.status_line_with_runway(at, now, by, &runway)
    }

    /// [`Self::status_line`] with the runway text given by the caller.
    fn status_line_with_runway(
        &self,
        at: i64,
        now: i64,
        by: Option<Writer>,
        runway: &str,
    ) -> String {
        let age = now.saturating_sub(at).max(0);
        let age = match age {
            a if a < 60 => format!("{a}s"),
            a if a < 3600 => format!("{}m", a / 60),
            a if a < 86_400 => format!("{}h", a / 3600),
            a => format!("{}d", a / 86_400),
        };
        format!(
            "capacity: headroom {}, usable {}{}, runway {} (as of {}, {age} ago{})",
            self.headroom_text(),
            self.usable_text(),
            match self.unknown.len() {
                0 => String::new(),
                m => format!(", {m} unknown"),
            },
            runway,
            iso(at),
            match by {
                Some(Writer::Usage) => ", profile usage: all logins",
                Some(Writer::Rotate) => ", rotate cycle: probed profiles only",
                None => "",
            }
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
