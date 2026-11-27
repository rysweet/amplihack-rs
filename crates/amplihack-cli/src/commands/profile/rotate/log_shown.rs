//! What the rotate log shows for each login (issue #1844): the windows as
//! read, so an exhausted login keeps its weekly figure. Display only: no
//! decision reads it.

use super::super::cycle::Ctx;
use super::super::decide::target_msg::annotate;
use super::super::decide::{Probed, Reading};
use super::super::fresh::{MAX_READING_AGE_SECS, read_at};
use super::super::observe::{Measurement, Snap, live_name};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::{usage_history, usage_passive};

/// What the log shows for one login (issue #1844): the reading the
/// decision used, and the session and weekly percentages as read. Display
/// only: no decision reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub reading: Reading,
    pub session: Option<f64>,
    pub week: Option<f64>,
    /// The deciding model; `None` denotes the all-models week.
    pub week_model: Option<String>,
    /// The read had not finished when the cycle stopped waiting for it.
    pub still_running: bool,
    /// Issue #2218: the live login of an excluded profile, never probed.
    pub excluded: bool,
    /// Age in whole seconds when the cycle shows figures from a prior
    /// observation.
    pub cached: Option<u64>,
    /// Issue #2910 (D11): why the probe said nothing about the login's room,
    /// shown as `unknown, <label>` in place of the figures.
    pub label: Option<&'static str>,
    /// Issue #2910 (D11): what follows the figures (a block, a week above the
    /// ceiling, the age of a last good reading), or the label.
    pub note: Option<String>,
}

impl Shown {
    /// The reading and the windows of one probe.
    pub fn of(p: &Probed) -> Self {
        let week = p.windows.deciding_week();
        Self {
            reading: p.reading,
            session: p.windows.session.map(|w| w.used),
            week: week.map(|w| w.used),
            week_model: week.and_then(|w| w.model.map(str::to_owned)),
            still_running: p.still_running,
            excluded: false,
            cached: None,
            label: None,
            note: None,
        }
    }

    /// A reading with no windows beyond what it holds itself.
    pub fn from_reading(reading: Reading) -> Self {
        let (session, week) = match reading {
            Reading::Measured { session, week } => (Some(session), Some(week)),
            Reading::Exhausted { session, week } => (session, week),
            Reading::Failed => (None, None),
        };
        Self {
            reading,
            session,
            week,
            week_model: None,
            still_running: false,
            excluded: false,
            cached: None,
            label: None,
            note: None,
        }
    }

    /// Issue #2889: `<sep>cached=<n>s` when the figures this reading shows
    /// were read before the cycle's measurement began, else nothing. Fixed
    /// text and one whole number, in the `s=`, `w=` and `reason=` grammar of
    /// the rest of the line.
    pub fn age(&self, sep: &str) -> String {
        self.cached
            .map_or_else(String::new, |n| format!("{sep}cached={n}s"))
    }
}

/// Issue #2889: how old the live reading `live` was at `now`, in whole
/// seconds, when it was read before the cycle began measuring at
/// `measured_at`; `None` otherwise.
///
/// A direct probe and a successful re-read are both dated at or after
/// `measured_at`, so neither is annotated, and a reading with no reading
/// time (carried, unknown, still running or failed, `fresh::read_at`) gives
/// no age rather than a guessed one. The difference is taken with saturating
/// arithmetic on `i64` and a reading time in the future reports `0`, so no
/// age is ever negative or wrapped.
fn cached_age(live: &Probed, measured_at: i64, now: i64) -> Option<u64> {
    let read = read_at(live, measured_at)?;
    if read >= measured_at {
        return None;
    }
    Some(u64::try_from(now.saturating_sub(read)).unwrap_or(0))
}

/// What the log shows for the live login and each candidate (issue #1844):
/// the windows read when the measurement kept its probes, else the
/// readings alone. `live_excluded` (issue #2218, `Snap::live_excluded`)
/// shows the live login as `excluded (not probed)`.
///
/// `now` is the cycle's own clock, the clock that dates the readings, not
/// the wall clock the log line is stamped with. Issue #2889: it gives the
/// live login's `cached=<n>s`, which no candidate ever carries.
///
/// Issue #2910 (D11): each candidate carries the reason the cycle saw for it
/// (`target_msg::annotate`): an unknown reading with its label and any last
/// good reading, or an active block or a week above the ceiling.
pub fn shown(m: &Measurement, live_excluded: bool, now: i64) -> (Shown, Vec<(ProfileName, Shown)>) {
    let (mut live, mut others) = shown_all(m);
    for (n, s) in &mut others {
        annotate(&m.seen, n, s);
    }
    live.excluded = live_excluded;
    live.cached = m
        .probed
        .as_ref()
        .and_then(|(p, _)| cached_age(p, m.measured_at, now));
    (live, others)
}

/// Adds the causes and last good figures shown on the cycle line.
pub(in crate::commands::profile::rotate) fn cycle_shown(
    ctx: &Ctx,
    snap: &Snap,
    m: &Measurement,
    now: i64,
) -> (Shown, Vec<(ProfileName, Shown)>) {
    let (mut live, mut others) = shown(m, snap.live_excluded, now);
    let root = ctx.store.root();
    let mut history = None;
    let mut cached_figures = |name: &ProfileName| {
        let history = history.get_or_insert_with(|| usage_history::last_observations(root, now));
        let report = usage_passive::last_good(root, name, now, history.get(name.as_str()))?;
        let figures = Shown::of(&Probed::from_report(&report));
        (figures.reading != Reading::Failed).then_some((report.observed_at?, figures))
    };
    if let Some(label) = m.seen.extra.live_label {
        let previous = live_name(&snap.live).and_then(|name| {
            let held = ctx
                .live_series
                .held(name, now)
                .filter(|held| held.at <= now)
                .map(|held| {
                    let figures = Shown::from_reading(Reading::Measured {
                        session: held.session,
                        week: held.week,
                    });
                    (held.at, figures)
                });
            cached_figures(name)
                .into_iter()
                .chain(held)
                .max_by_key(|(at, _)| *at)
        });
        let cached = m
            .probed
            .as_ref()
            .and_then(|(probe, _)| probe.observed_at)
            .filter(|&at| live.cached.is_some() && at <= now)
            .map(|at| (at, live.clone()));
        let previous = previous.into_iter().chain(cached).max_by_key(|(at, _)| *at);
        annotate_failure(&mut live, label.as_str(), previous, now);
    }
    for (name, shown) in &mut others {
        if shown.reading == Reading::Failed
            && let Some(label) = m.seen.extra.labels.get(name)
        {
            annotate_failure(shown, label.as_str(), cached_figures(name), now);
        }
    }
    (live, others)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::profile::rotate::measure::ProbeLabel;

    #[test]
    fn labelled_failure_shows_prior_figures_through_the_freshness_limit() {
        let at = 1_800_000_000;
        let label = ProbeLabel::Http429.as_str();
        let previous = Reading::Measured {
            session: 51.0,
            week: 80.0,
        };
        let failed = Probed::failed();
        let mut shown = Shown::of(&failed);
        annotate_failure(
            &mut shown,
            label,
            Some((at, Shown::from_reading(previous))),
            at + MAX_READING_AGE_SECS,
        );
        assert_eq!(shown.label, Some(label));
        assert_eq!(shown.cached, Some(MAX_READING_AGE_SECS as u64));
        assert_eq!((shown.session, shown.week), (Some(51.0), Some(80.0)));
        assert_eq!(failed.reading, Reading::Failed);
        assert_eq!(failed.observed_at, None);

        annotate_failure(
            &mut shown,
            label,
            Some((at, Shown::from_reading(previous))),
            at + MAX_READING_AGE_SECS + 1,
        );
        assert_eq!(shown.label, Some(label));
        assert_eq!(shown.cached, None);
        assert_eq!((shown.session, shown.week), (None, None));
    }
}

fn annotate_failure(
    shown: &mut Shown,
    label: &'static str,
    previous: Option<(i64, Shown)>,
    now: i64,
) {
    let cached = previous.and_then(|(at, mut figures)| {
        let age = now.checked_sub(at)?;
        if !(0..=MAX_READING_AGE_SECS).contains(&age) {
            return None;
        }
        figures.cached = Some(age as u64);
        Some(figures)
    });
    *shown = cached.unwrap_or_else(|| Shown::from_reading(Reading::Failed));
    shown.label = Some(label);
}

fn shown_all(m: &Measurement) -> (Shown, Vec<(ProfileName, Shown)>) {
    match &m.probed {
        Some((live, probes)) => (
            Shown::of(live),
            probes
                .iter()
                .map(|(n, p)| (n.clone(), Shown::of(p)))
                .collect(),
        ),
        None => (
            Shown::from_reading(m.live),
            m.others
                .iter()
                .map(|(n, r)| (n.clone(), Shown::from_reading(*r)))
                .collect(),
        ),
    }
}

/// The shown reading of `name`, `failed` when it was not measured.
pub fn shown_of(others: &[(ProfileName, Shown)], name: &ProfileName) -> Shown {
    others
        .iter()
        .find(|(n, _)| n == name)
        .map_or_else(|| Shown::of(&Probed::failed()), |(_, s)| s.clone())
}

/// A percentage as a whole number, rounded down; 0 for anything below 0 or
/// not finite. The one rounding of every rotate line, `msg::usage` and the
/// issue #2910 lines (`decide::target_msg`) alike.
pub(in crate::commands::profile::rotate) fn pct(p: f64) -> u64 {
    if p.is_finite() && p > 0.0 {
        p.floor() as u64
    } else {
        0
    }
}

/// `s=<n>% w=<n>%` with `sep` between, from the windows read (#1844):
/// exhausted shows its stored figures; no session window is `s=0%`, as
/// in D1 (#1846), so a weekly-only login never logs a session limit it
/// does not have. An unread week is `w=?`, failed `failed`, in flight
/// `unknown, still running`. Issue #2889: a measured or exhausted
/// reading read before this cycle's measurement began ends
/// `{sep}cached=<n>s` (`Shown::age`).
pub(in crate::commands::profile::rotate) fn usage(r: &Shown, sep: &str) -> String {
    if r.excluded {
        return excluded::EXCLUDED_NOT_PROBED.to_string();
    }
    // Issue #2910 (D11): an unknown reading with its label, and its last
    // good reading when one stood in for it.
    if let Some(label) = r.label {
        if r.cached.is_some() {
            return format!("unknown, {label}, last {}", usage_figures(r, sep));
        }
        return match &r.note {
            Some(n) => format!("unknown, {label}, {n}"),
            None => format!("unknown, {label}"),
        };
    }
    if r.still_running {
        return "unknown, still running".to_string();
    }
    usage_figures(r, sep)
}

fn usage_figures(r: &Shown, sep: &str) -> String {
    let note = r
        .note
        .as_ref()
        .map_or_else(String::new, |n| format!(" {n}"));
    let week = |w: Option<f64>| w.map_or_else(|| "?".to_string(), |w| format!("{}%", pct(w)));
    let (session, w) = match r.reading {
        Reading::Measured { session, .. } => (r.session.unwrap_or(session), r.week),
        Reading::Exhausted { session, week: w } => {
            (session.or(r.session).unwrap_or(0.0), w.or(r.week))
        }
        Reading::Failed => return "failed".to_string(),
    };
    let origin = r
        .week_model
        .as_ref()
        .map_or_else(String::new, |m| format!("({m})"));
    format!(
        "s={}%{sep}w={}{origin}{}{note}",
        pct(session),
        week(w),
        r.age(sep)
    )
}

/// Issue #2218: what the rotate log says about an excluded profile, which
/// the cycle never probes or refreshes. Re-exported through `log::msg`.
pub mod excluded {
    use crate::commands::profile::name::ProfileName;

    /// Issue #2218: how the log shows an excluded live login's reading.
    pub const EXCLUDED_NOT_PROBED: &str = "excluded (not probed)";

    /// Issue #2218: `<root>/.auto-switch` cannot be read or is unsafe
    /// (another user's, or writable by group or others), so which logins
    /// are excluded is unknown and the cycle stops before measuring.
    pub const EXCLUDE_PAUSED: &str =
        "auto-switch config unreadable or unsafe; rotation paused, no profile probed";

    /// Issue #2218: the live login is excluded; the cycle swaps off it.
    pub fn excluded_moving_off(live: &ProfileName) -> String {
        format!("note: live profile {live} is excluded; moving off it")
    }

    /// Issue #2218: the live login is excluded and no profile can take it.
    pub fn alert_excluded_live(live: &ProfileName) -> String {
        format!("ALERT: live profile {live} is excluded; no eligible candidate")
    }

    /// Issue #2218 (S4): only `~/.claude.json` names the excluded live
    /// login, which may be stale, so it is not moved into that folder.
    pub fn alert_excluded_unproven(live: &ProfileName) -> String {
        format!(
            "ALERT: live profile {live} is excluded; not moved, as only ~/.claude.json names it"
        )
    }

    /// Issue #2218: the expired live login is excluded, so it is left as it
    /// is: a refresh would retire the token its other holders use.
    pub fn excluded_not_refreshed(live: &ProfileName) -> String {
        format!("refresh: live profile {live} is excluded; token not refreshed")
    }

    /// Issue #2218: a pin holds an excluded overflow profile live, so the
    /// held cycle runs no spend probe on it.
    pub fn pinned_excluded(name: &ProfileName) -> String {
        format!("skip: pinned profile {name} is excluded; spend not probed")
    }
}
