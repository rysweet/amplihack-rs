//! Issue #2211: the live login's reading series, kept in memory across
//! watch cycles by `step` in `live_fresh.rs`. Pure state and functions of
//! their arguments: no file, process or clock.
//!
//! A [`Point`] is a reading time, a session figure and the session window's
//! reset time. Session use only rises within one window, so the series tells
//! a new window from a stale figure by the reset time ([`window`]), never by
//! the observation time: the status line rewrites old figures with new
//! times, and many sessions write the same cache. [`contradicts`] says when
//! a cached figure cannot be current next to the last accepted point. The
//! series also keeps the frozen count on the measured figures and the mark
//! that a re-read found the cache behind. It holds a profile name, times and
//! percentages only. See "Live reading series" in
//! docs/reference/profile-command.md.

use super::super::super::decide::{Probed, Reading};
use super::super::read_at;
use crate::commands::profile::name::ProfileName;
use std::collections::VecDeque;

/// The most points a series keeps.
pub const MAX_POINTS: usize = 4;

/// The length of a session window. A figure read at `at` belongs to a
/// window that has ended by `at + SESSION_WINDOW_SECS` at the latest.
pub const SESSION_WINDOW_SECS: i64 = 5 * 3600;

/// Session reset times at most this far apart name the same window. A
/// direct read prints the reset to the minute or the hour ("Resets 5pm"),
/// and the status line stores it to the second, so the two can differ by
/// up to an hour. The next window starts no earlier than this one's reset
/// and lasts [`SESSION_WINDOW_SECS`], so its reset is hours later.
pub const SAME_WINDOW_SECS: i64 = 3600;

/// One live reading: when it was read, its session use, and when its
/// session window resets (`None` when the reading gave no reset time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub at: i64,
    pub session: f64,
    pub resets: Option<i64>,
}

/// How a reading's session window compares with a point's, by reset time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// A later window: its reset is more than [`SAME_WINDOW_SECS`] later.
    Later,
    Same,
    /// An earlier window, already over when the point was read.
    Earlier,
    /// One reset time or both are unknown.
    Unknown,
}

/// The window of a reading whose session resets at `new`, next to a point
/// whose session resets at `last`.
pub fn window(last: Option<i64>, new: Option<i64>) -> Window {
    match (last, new) {
        (Some(l), Some(n)) if n > l.saturating_add(SAME_WINDOW_SECS) => Window::Later,
        (Some(l), Some(n)) if n < l.saturating_sub(SAME_WINDOW_SECS) => Window::Earlier,
        (Some(_), Some(_)) => Window::Same,
        _ => Window::Unknown,
    }
}

/// Whether a figure `session`, in the window that resets at `resets`,
/// cannot be current next to the accepted point `last`. Use never falls
/// within one window, so a lower figure in the same window is stale. So is
/// a lower one whose window is unknown, until a direct read confirms it,
/// and any figure from an earlier window. A newer observation time proves
/// nothing here: the status line rewrites old figures with new times.
pub fn contradicts(last: &Point, session: f64, resets: Option<i64>) -> bool {
    match window(last.resets, resets) {
        Window::Later => false,
        Window::Earlier => true,
        Window::Same | Window::Unknown => session < last.session,
    }
}

/// When the session window of probe `p` resets, if it gave a reset time.
pub(super) fn session_resets(p: &Probed) -> Option<i64> {
    p.windows.session.and_then(|w| w.resets_at)
}

/// The live login's recent readings, kept in memory across watch cycles.
#[derive(Debug, Default)]
pub struct Series {
    name: Option<ProfileName>,
    points: VecDeque<Point>,
    /// The last cycle's measured session and week figures, for the frozen
    /// count. Always the measurement's own figures (cached or direct), never
    /// a re-read's.
    prev: Option<(f64, f64)>,
    unchanged: u32,
    /// The last re-read disagreed with the measured figures: the cached
    /// source is behind, so a measured figure below the last point is stale,
    /// not a new session window.
    behind: bool,
}

#[cfg(test)]
#[path = "tests_live_fresh_access.rs"]
mod access;

/// Whether a reading taken at `a` is too old, or later than the clock `t`.
pub(super) fn stale_at(a: i64, t: i64, interval: i64) -> bool {
    a < t.saturating_sub(interval) || a > t
}

impl Series {
    /// Starts afresh for `name` when another login owned the series.
    fn own(&mut self, name: &ProfileName) {
        if self.name.as_ref() != Some(name) {
            self.clear();
            self.name = Some(name.clone());
        }
    }

    /// Adds `(at, session)`, in the session window that resets at
    /// `resets`, for live login `name` at clock `t`; see the contract in
    /// `tests_issue_2211`.
    ///
    /// A later window clears the series first: the old points say nothing
    /// of it. A figure from an earlier window, or a lower one in the same
    /// window, is stale and is not added. With no reset time to compare, a
    /// lower figure clears the series: `step` reads a lower cached figure
    /// again first, so only a direct read reaches here with one.
    pub fn note(&mut self, name: &ProfileName, at: i64, session: f64, resets: Option<i64>, t: i64) {
        if !session.is_finite() || at > t {
            return;
        }
        self.own(name);
        if let Some(last) = self.points.back() {
            if at <= last.at {
                return;
            }
            let lower = session < last.session;
            match window(last.resets, resets) {
                Window::Later => self.points.clear(),
                Window::Earlier => return,
                Window::Same if lower => return,
                Window::Unknown if lower => self.points.clear(),
                Window::Same | Window::Unknown => {}
            }
        }
        self.points.push_back(Point {
            at,
            session,
            resets,
        });
        while self.points.len() > MAX_POINTS {
            self.points.pop_front();
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Session points per second from the first point to the last, when
    /// they are at least `min_span` seconds apart (and at least one second).
    ///
    /// The rotator passes `--interval`. Two readings a few seconds apart, a
    /// direct read and a status line written just after it, say little
    /// about the rate, and a one-point rise over five seconds would project
    /// 24 points ahead over a 120-second interval. Over a span of at least
    /// one interval, `rate * interval` is never more than the rise the
    /// series has seen, so the projection never adds more than that.
    pub fn rate(&self, min_span: i64) -> Option<f64> {
        let (first, last) = (self.points.front()?, self.points.back()?);
        let dt = last.at.checked_sub(first.at)?;
        if dt <= 0 || dt < min_span {
            return None;
        }
        let r = (last.session - first.session) / dt as f64;
        r.is_finite().then_some(r)
    }

    /// The last point, when the series belongs to `name`.
    pub(super) fn last_for(&self, name: &ProfileName) -> Option<Point> {
        let last = self.points.back().copied();
        last.filter(|_| self.name.as_ref() == Some(name))
    }

    /// The last point for `name` while the session window it was read in
    /// can still be open at `t`: before its reset time when that is known,
    /// and less than [`SESSION_WINDOW_SECS`] after it was read either way.
    /// Only then is its figure a floor on the login's use now, so only then
    /// may it stand in for a failed or stale reading.
    ///
    /// Not `fresh::MAX_READING_AGE_SECS` (300 s): the default `--interval`
    /// is 600 s, so the previous cycle's point would always be too old.
    pub fn last_known(&self, name: &ProfileName, t: i64) -> Option<Point> {
        self.last_for(name).filter(|p| {
            t.saturating_sub(p.at) < SESSION_WINDOW_SECS && p.resets.is_none_or(|r| t < r)
        })
    }

    /// Records this cycle's figures for `name` and returns how many cycles
    /// in a row the cached (passive) source has shown them unchanged.
    ///
    /// A direct reading (`passive` false) restarts the count. It was read
    /// from the usage source in this cycle, so it is current, as a re-read
    /// that agrees shows (`after_reread`); identical direct figures mean an
    /// idle login, and reading them again by the same method could tell
    /// nothing new. So the first cached figure after a run of direct
    /// readings starts a new count, and is not re-read because direct
    /// readings before it matched it.
    pub(super) fn seen(
        &mut self,
        name: &ProfileName,
        session: f64,
        week: f64,
        passive: bool,
    ) -> u32 {
        self.own(name);
        self.unchanged = match self.prev {
            Some(p) if passive && p == (session, week) => self.unchanged.saturating_add(1),
            _ => 0,
        };
        self.prev = Some((session, week));
        self.unchanged
    }

    /// After a live re-read that read `figures` (`None` when it failed, read
    /// no figures or could not be made) of a measurement that read
    /// `measured`. A re-read that agrees restarts the frozen count: the
    /// source is idle, not frozen. One that disagrees marks the source as
    /// behind and keeps the count, so the next identical measurement is read
    /// again. A failed one changes nothing: the count and the mark stay as
    /// they were, so a frozen reading is read again on the next identical
    /// cycle, as a lagging one is. A failure proves nothing about the cache,
    /// so it must not make a frozen figure trusted again. The count stays on
    /// the measured figures either way.
    pub(super) fn after_reread(&mut self, measured: (f64, f64), figures: Option<(f64, f64)>) {
        match figures {
            Some(f) if f == measured => {
                self.unchanged = 0;
                self.behind = false;
            }
            Some(_) => self.behind = true,
            None => {}
        }
    }

    /// Lag ends only when a current cached figure no longer contradicts the
    /// last accepted point ([`contradicts`]): it has caught up within the
    /// same window, or it is from a later window by its session reset time.
    /// A newer observation time alone never ends it: the status line
    /// rewrites old figures with new times, so a lower figure stamped later
    /// in the same window is still behind, and is read again.
    pub(super) fn still_behind(
        &mut self,
        name: &ProfileName,
        probe: &Probed,
        measured_at: i64,
        t: i64,
        interval: i64,
    ) -> bool {
        if self.behind
            && let Some(last) = self.last_for(name)
            && let Reading::Measured { session, .. } = probe.reading
            && let Some(at) = read_at(probe, measured_at)
            && !stale_at(at, t, interval)
            && at >= last.at
            && !contradicts(&last, session, session_resets(probe))
        {
            self.behind = false;
        }
        self.behind
    }

    /// The last cycle's measured week figure, for a substituted outlook.
    pub(super) fn week(&self) -> Option<f64> {
        self.prev.map(|(_, w)| w)
    }
}
