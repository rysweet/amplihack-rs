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
//!   counts its rises in `w` within each week. Issue #3258: a new week
//!   starts only when `w_reset` passes or moves later by more than
//!   [`SAME_WEEK_SLACK`]; a fall alone is never a reset. A fall that a
//!   later reading in the week reverses is a stale reading and is skipped; a
//!   fall that stays lowers the level and adds no use, so the rate is never
//!   negative. Readings from a week already over, and a stale passive
//!   reading now, are left out. Each login's use is weighted.
//! - Runway: weighted headroom over the rate (`runway_hours`). Issue #3258:
//!   the printed runway also allows for the weekly renewals of the next 24 h.
//!   It steps from renewal to renewal, taking away the rate and
//!   adding each renewal, and gives the time the room first reaches zero,
//!   or says the room lasts beyond 24 h with the lowest room on the way.
//!   With no renewal listed it is the plain runway.
//! - `rotate status` takes no runway from a rotator line that left logins
//!   out; see [`status_line`].
//! - Rounding is half away from zero and happens only when printed; a value
//!   that is not finite prints as `null`.

use super::capacity_weight::Weights;
use super::usage_cmd::{sanitize_display, utc_time as iso};
use super::usage_history::{Entry, Point, SAME_WEEK_SLACK, Snapshot, Writer, stale};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;

/// How far back the rate looks.
pub(super) const RATE_WINDOW: i64 = 6 * 3600;
/// The youngest start the rate accepts.
const RATE_MIN_SPAN: i64 = 4 * 3600 + 30 * 60;
/// Issue #3258: a reading below a login's level is a stale one when a later
/// reading in the same week comes back to within this many points of the
/// level. A fall is never a reset on its own.
pub(super) const STALE_MARGIN: u8 = 10;
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
    /// Issue #3258: the runway with the renewals of the next 24 h.
    renewals: Option<Walk>,
    resets: Vec<WeeklyReset>,
}

/// Issue #3258: the room stepped through the next 24 h at the rate, with
/// each listed weekly renewal added at its time.
#[derive(Debug, Clone, PartialEq)]
struct Walk {
    /// Hours until the room first reaches zero; `None` when it lasts the
    /// 24 h and some renewal falls inside them.
    hours: Option<f64>,
    /// Lowest weighted room on the way, in login-weeks.
    lowest: f64,
}

/// The [`Walk`] from weighted `room` (login-weeks) at `rate` (login-weeks
/// an hour, above 0) through `resets`, sorted by time, after `now`.
///
/// With no renewal listed the runway is the plain `room / rate`, even past
/// 24 h. Otherwise the room is taken down to each renewal and checked before
/// the renewal is added; after the last one it runs on to 24 h.
fn walk(room: f64, rate: f64, resets: &[WeeklyReset], now: i64) -> Walk {
    let horizon = RESETS_AHEAD as f64 / 3600.0;
    if resets.is_empty() {
        return Walk {
            hours: Some(room / rate),
            lowest: (room - rate * horizon).max(0.0),
        };
    }
    let mut t = 0.0;
    let mut room = room;
    let mut lowest = room;
    let stops = resets
        .iter()
        .map(|r| ((r.at - now) as f64 / 3600.0, r.restores))
        .chain(std::iter::once((horizon, 0.0)));
    for (at, restores) in stops {
        let before = room - rate * (at - t);
        if before <= 0.0 {
            return Walk {
                hours: Some(t + room / rate),
                lowest: 0.0,
            };
        }
        lowest = lowest.min(before);
        room = before + restores;
        t = at;
    }
    Walk {
        hours: None,
        lowest,
    }
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
/// taken at `b_at`: `a`'s reset time has passed, or `b`'s reset time is
/// later than `a`'s by more than [`SAME_WEEK_SLACK`], which is how a week
/// reset early shows. Issue #3258: a fall in `w` is not a reset on its own;
/// without reset times nothing is inferred.
fn reset_between(a: &Point, b: &Point, b_at: i64) -> bool {
    a.w_reset.is_some_and(|r| r <= b_at)
        || matches!((a.w_reset, b.w_reset), (Some(ra), Some(rb)) if rb.saturating_sub(ra) > SAME_WEEK_SLACK)
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
/// Issue #3258: each week keeps a level, the start's figure for the first
/// week and 0 for each later one. A reading above the level is use, and
/// raises the level. A reading below it is stale when a later reading in
/// the same week comes back to within [`STALE_MARGIN`] points of the level,
/// and is skipped. Otherwise the fall stays: it lowers the level to the new
/// figure and adds no use. Never negative.
fn used_over(readings: &[(i64, &Point)]) -> i64 {
    let mut weeks: Vec<Vec<i64>> = Vec::new();
    let mut prev: Option<&Point> = None;
    for (at, p) in readings {
        match (prev, weeks.last_mut()) {
            (Some(a), Some(week)) if !reset_between(a, p, *at) => week.push(i64::from(p.w)),
            _ => {
                // A later week counts from 0: a leading 0 stands for it.
                let start = if prev.is_some() { vec![0] } else { Vec::new() };
                weeks.push(start);
                if let Some(week) = weeks.last_mut() {
                    week.push(i64::from(p.w));
                }
            }
        }
        prev = Some(*p);
    }
    weeks.iter().map(|w| used_in_week(w)).sum()
}

/// Use within one week of figures, oldest first; see [`used_over`].
fn used_in_week(figures: &[i64]) -> i64 {
    let Some((&first, rest)) = figures.split_first() else {
        return 0;
    };
    let margin = i64::from(STALE_MARGIN);
    let mut level = first;
    let mut used = 0i64;
    for (i, &w) in rest.iter().enumerate() {
        if w > level {
            used = used.saturating_add(w - level);
            level = w;
        } else if w < level && !rest[i + 1..].iter().any(|&later| later >= level - margin) {
            level = w;
        }
    }
    used
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
    let positive = rate.filter(|r| *r > 0.0);
    let runway = positive.map(|r| weighted_points / 100.0 / r);
    let renewals = positive.map(|r| walk(weighted_points / 100.0, r, &resets, i.now));

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
        renewals,
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
            let h = summary_of(e, earlier, weights, threshold).runway_short()?;
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

    /// The runway with renewals: `~H h`, or `beyond 24 h` when the room
    /// lasts the next 24 h; `None` when the runway is unknown.
    fn runway_text(&self) -> Option<String> {
        let walk = self.renewals.as_ref()?;
        Some(match walk.hours {
            Some(h) => format!("~{} h", fixed(h, 0)),
            None => "beyond 24 h".to_string(),
        })
    }

    /// [`Self::runway_text`] for the `rotate status` line, with the lowest
    /// room when the room lasts beyond 24 h.
    fn runway_short(&self) -> Option<String> {
        let text = self.runway_text()?;
        Some(match self.renewals.as_ref() {
            Some(Walk {
                hours: None,
                lowest,
            }) => {
                format!("{text} (lowest {} login-weeks)", fixed(*lowest, 2))
            }
            _ => text,
        })
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
        let runway = match (self.runway_text(), self.rate, &self.renewals) {
            (Some(h), Some(rate), Some(walk)) if walk.hours.is_none() => format!(
                "{h} at the last 6 h rate ({} login-weeks/h; lowest {} login-weeks, \
                 with the resets below)",
                fixed(rate, 3),
                fixed(walk.lowest, 2)
            ),
            (Some(h), Some(rate), _) => {
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
        let runway = self.runway_short().unwrap_or_else(|| "unknown".to_string());
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
            runway_with_renewals_hours: self
                .renewals
                .as_ref()
                .and_then(|w| w.hours)
                .and_then(|h| rounded(h, 1)),
            runway_beyond_24h: self.renewals.as_ref().is_some_and(|w| w.hours.is_none()),
            lowest_room_login_weeks: self.renewals.as_ref().and_then(|w| rounded(w.lowest, 2)),
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
    runway_with_renewals_hours: Option<f64>,
    runway_beyond_24h: bool,
    lowest_room_login_weeks: Option<f64>,
    weekly_resets_next_24h: Vec<WeeklyResetJson<'a>>,
}

impl Serialize for Summary {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.json().serialize(s)
    }
}
