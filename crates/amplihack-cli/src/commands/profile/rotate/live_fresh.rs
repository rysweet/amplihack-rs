//! Issue #2211: the live login's reading freshness and the projected swap.
//! Not to be confused with `live_refresh.rs` (#1753), which refreshes the
//! live login's credentials; this module only reads usage again and never
//! touches a credential.
//!
//! Once per cycle, after the measurement and before the target gate,
//! [`step`]:
//!
//! - reads the live login again when its reading is frozen (the same session
//!   and week figures on [`FROZEN_CYCLES`] cycles after the first, with a
//!   session above 0) or older than one `--interval` (R1, R1b);
//! - projects the session use one interval ahead from a short in-memory
//!   [`Series`], and decides on the projection when it reaches
//!   `--session-at` (R2);
//! - decides a failed or still-stale live reading on the larger of
//!   `--session-at` and the last known session use when the last known
//!   point was near the threshold (R2b), so the figure never falls.
//!
//! The figures only change the decision, through [`Outlook`] on the cycle's
//! context, which `decide_measured` reads; the ledger, the history and the
//! log lines keep the reading as measured. [`Point`], [`Series`] and
//! [`Outlook`] hold a profile name, times and percentages only: never a
//! token, an account id, an email or a path. Nothing here is persisted.
//! See "Rotate live reading freshness" in docs/reference/profile-command.md.

use super::super::cycle::Ctx;
use super::super::decide::{Decision, Probed, Reading, Reason, needs_swap};
use super::super::fresh_msg;
use super::super::identify::LiveId;
use super::super::measure;
use super::super::observe::{Measurement, Snap, decide_measured};
use super::super::settings::Settings;
use super::read_at;
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use std::collections::VecDeque;

/// The most points a series keeps.
pub const MAX_POINTS: usize = 4;

/// Unchanged cycles after which a live reading is frozen: the third
/// identical reading in a row.
pub const FROZEN_CYCLES: u32 = 2;

/// One live reading: when it was read, and its session use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub at: i64,
    pub session: f64,
}

/// The live login's recent readings, kept in memory across watch cycles.
#[derive(Debug, Default)]
pub struct Series {
    name: Option<ProfileName>,
    points: VecDeque<Point>,
    /// The last cycle's session and week figures, for the frozen count.
    prev: Option<(f64, f64)>,
    unchanged: u32,
}

impl Series {
    /// Starts afresh for `name` when another login owned the series.
    fn own(&mut self, name: &ProfileName) {
        if self.name.as_ref() != Some(name) {
            self.clear();
            self.name = Some(name.clone());
        }
    }

    /// Adds `(at, session)` for live login `name` at clock `t`; see the
    /// contract in `tests_issue_2211`.
    pub fn note(&mut self, name: &ProfileName, at: i64, session: f64, t: i64) {
        if !session.is_finite() || at > t {
            return;
        }
        self.own(name);
        if let Some(last) = self.points.back() {
            if at <= last.at {
                return;
            }
            if session < last.session {
                // A new session window: the old points say nothing of it.
                self.points.clear();
            }
        }
        self.points.push_back(Point { at, session });
        while self.points.len() > MAX_POINTS {
            self.points.pop_front();
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Session points per second from the first point to the last.
    pub fn rate(&self) -> Option<f64> {
        let (first, last) = (self.points.front()?, self.points.back()?);
        let dt = last.at.checked_sub(first.at)?;
        if dt <= 0 {
            return None;
        }
        let r = (last.session - first.session) / dt as f64;
        r.is_finite().then_some(r)
    }

    /// The last point, when the series belongs to `name`.
    fn last_for(&self, name: &ProfileName) -> Option<Point> {
        let last = self.points.back().copied();
        last.filter(|_| self.name.as_ref() == Some(name))
    }

    /// Records this cycle's figures for `name` and returns how many cycles
    /// in a row they have been unchanged.
    fn seen(&mut self, name: &ProfileName, session: f64, week: f64) -> u32 {
        self.own(name);
        self.unchanged = match self.prev {
            Some(p) if p == (session, week) => self.unchanged.saturating_add(1),
            _ => 0,
        };
        self.prev = Some((session, week));
        self.unchanged
    }

    /// After a live re-read: its figures, when it read any, and a frozen
    /// count from 0.
    fn restart_count(&mut self, figures: Option<(f64, f64)>) {
        if figures.is_some() {
            self.prev = figures;
        }
        self.unchanged = 0;
    }
}

/// `session + rate * interval`, limited to `[0, 100]`.
pub fn projected(session: f64, rate: f64, interval: u64) -> f64 {
    (session + rate * interval as f64).clamp(0.0, 100.0)
}

/// Whether a last known session figure `last` is near `--session-at`.
pub fn near(last: f64, rate: Option<f64>, s: &Settings) -> bool {
    let at = f64::from(s.session_at);
    match rate {
        Some(r) => last + r * s.interval as f64 >= at,
        None => last >= at - f64::from(s.margin),
    }
}

/// What the live figures used by the decision rest on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// The reading as measured.
    Real,
    /// The session use projected one interval ahead.
    Projected,
    /// The larger of `--session-at` and the last known session use,
    /// standing in for a failed or stale reading near the threshold.
    Substituted,
}

/// The live figures this cycle's decision uses, for live login `name`.
#[derive(Debug, Clone, PartialEq)]
pub struct Outlook {
    pub name: ProfileName,
    pub session: f64,
    pub week: f64,
    pub basis: Basis,
}

/// The live probe the decision runs on: `probe`, or a measured reading with
/// the outlook's figures.
pub fn decision_input(probe: &Probed, outlook: Option<&Outlook>) -> (Probed, Basis) {
    let Some(o) = outlook else {
        return (probe.clone(), Basis::Real);
    };
    let mut p = probe.clone();
    p.reading = Reading::Measured {
        session: o.session.clamp(0.0, 100.0),
        week: o.week,
    };
    p.unknown = false;
    p.still_running = false;
    (p, o.basis)
}

/// Relabels a threshold swap made on an outlook as `projected` only when
/// the `real` reading would not itself have needed a swap; a failed real
/// reading under a substitution counts as not needing one. Never creates,
/// removes or retargets a swap.
pub fn map_reason(d: Decision, basis: Basis, s: &Settings, real: &Reading) -> Decision {
    let relabel = match basis {
        Basis::Real => false,
        Basis::Substituted if *real == Reading::Failed => true,
        _ => !needs_swap(s, real),
    };
    match d {
        Decision::Swap {
            to,
            reason: Reason::Threshold,
        } if relabel => Decision::Swap {
            to,
            reason: Reason::Projected,
        },
        d => d,
    }
}

/// The live freshness step for `m`, decided on `snap`. Sets
/// `ctx.live_outlook`, updates `ctx.live_series`, makes at most one live
/// re-read (logged as [`fresh_msg::STALE_LIVE`]) and decides again when
/// anything changed. The failed live probe count is unchanged.
pub(in crate::commands::profile::rotate) fn step(
    ctx: &mut Ctx,
    snap: &Snap,
    mut m: Measurement,
) -> Result<Measurement, ProfileError> {
    ctx.live_outlook = None;
    let name = match &snap.live {
        LiveId::Known(n) if !snap.platform_live() => n.clone(),
        _ => {
            ctx.live_series.clear();
            return Ok(m);
        }
    };
    // A stub that built the measurement by hand kept no probes.
    let Some((probe, probes)) = m.probed.clone() else {
        return Ok(m);
    };
    let t = (ctx.now)();
    let interval = i64::try_from(ctx.settings.interval).unwrap_or(i64::MAX);
    let mut live = probe.clone();
    let mut still_stale = false;
    if let Reading::Measured { session, week } = probe.reading {
        let unchanged = ctx.live_series.seen(&name, session, week);
        let frozen = session > 0.0 && unchanged >= FROZEN_CYCLES;
        let old = read_at(&probe, m.measured_at).is_none_or(|a| stale_at(a, t, interval));
        if frozen || old {
            match reread_live(ctx, &name, t, interval, &mut m.fresh_lines) {
                Some(p) => {
                    let figures = match p.reading {
                        Reading::Measured { session, week } => Some((session, week)),
                        _ => None,
                    };
                    ctx.live_series.restart_count(figures);
                    live = p;
                }
                None => {
                    ctx.live_series.restart_count(None);
                    still_stale = true;
                }
            }
        }
    }
    let s = &ctx.settings;
    let at = f64::from(s.session_at);
    let basis = match live.reading {
        Reading::Measured { session, week } if !still_stale => {
            if let Some(read) = read_at(&live, m.measured_at) {
                ctx.live_series.note(&name, read, session, t);
            }
            ctx.live_series
                .rate()
                .filter(|r| *r > 0.0 && session < at)
                .map(|r| projected(session, r, s.interval))
                .filter(|p| *p >= at)
                .map(|p| (p, week, Basis::Projected))
        }
        Reading::Exhausted { .. } => None,
        // A failed probe, or a reading still stale after the step: never
        // decided on less than the last known session use.
        other => {
            let stale = match other {
                Reading::Measured { session, .. } => session,
                _ => at,
            };
            let week = ctx.live_series.prev.map(|(_, w)| w);
            let rate = ctx.live_series.rate();
            ctx.live_series
                .last_for(&name)
                .filter(|last| near(last.session, rate, s))
                .zip(week.filter(|w| w.is_finite()))
                .map(|(last, w)| (at.max(last.session).max(stale), w, Basis::Substituted))
        }
    };
    ctx.live_outlook = basis.map(|(session, week, basis)| Outlook {
        name,
        session,
        week,
        basis,
    });
    if ctx.live_outlook.is_none() && live == probe {
        return Ok(m);
    }
    let mut again = decide_measured(ctx, snap, live, probes, m.measured_at)?;
    debug_assert_eq!(
        again.failed, m.failed,
        "the live step counts the live probe once"
    );
    again.fresh_lines = std::mem::take(&mut m.fresh_lines);
    Ok(again)
}

/// Whether a reading taken at `a` is too old, or later than the clock `t`.
fn stale_at(a: i64, t: i64, interval: i64) -> bool {
    a < t.saturating_sub(interval) || a > t
}

/// One direct re-read of the live login `name`, after the
/// [`fresh_msg::STALE_LIVE`] line, timed at `t`. `None` when no read may be
/// made (no hook, or `rotate stop`), or, after a [`fresh_msg::failed`] line,
/// when the read failed or is still stale at `t`.
fn reread_live(
    ctx: &Ctx,
    name: &ProfileName,
    t: i64,
    interval: i64,
    lines: &mut Vec<String>,
) -> Option<Probed> {
    let reread = ctx.reread?;
    if ctx.stopping() {
        return None;
    }
    lines.push(fresh_msg::STALE_LIVE.to_string());
    let mut p = reread(ctx, name, measure::timeout_quiet());
    let failed = p.reading == Reading::Failed || p.unknown || p.still_running;
    if failed || p.observed_at.is_some_and(|a| stale_at(a, t, interval)) {
        lines.push(fresh_msg::failed(name));
        return None;
    }
    // A re-read is a direct read made in this cycle.
    p.observed_at = Some(t);
    p.carried = false;
    p.passive = false;
    Some(p)
}
