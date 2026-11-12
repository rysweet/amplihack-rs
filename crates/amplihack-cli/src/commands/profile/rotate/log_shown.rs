//! What the rotate log shows for each login (issue #1844): the windows as
//! read, so an exhausted login keeps its weekly figure. Display only: no
//! decision reads it.

use super::super::decide::{Probed, Reading};
use super::super::observe::Measurement;
use crate::commands::profile::name::ProfileName;

/// What the log shows for one login (issue #1844): the reading the
/// decision used, and the session and weekly percentages as read. Display
/// only: no decision reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub reading: Reading,
    pub session: Option<f64>,
    pub week: Option<f64>,
    /// The read had not finished when the cycle stopped waiting for it.
    pub still_running: bool,
    /// Issue #2218: the live login of an excluded profile, never probed.
    pub excluded: bool,
}

impl Shown {
    /// The reading and the windows of one probe.
    pub fn of(p: &Probed) -> Self {
        Self {
            reading: p.reading,
            session: p.windows.session.map(|w| w.used),
            week: p.windows.week.map(|w| w.used),
            still_running: p.still_running,
            excluded: false,
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
            still_running: false,
            excluded: false,
        }
    }
}

/// What the log shows for the live login and each candidate (issue #1844):
/// the windows read when the measurement kept its probes, else the
/// readings alone. `live_excluded` (issue #2218, `Snap::live_excluded`)
/// shows the live login as `excluded (not probed)`.
pub fn shown(m: &Measurement, live_excluded: bool) -> (Shown, Vec<(ProfileName, Shown)>) {
    let (mut live, others) = shown_all(m);
    live.excluded = live_excluded;
    (live, others)
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
