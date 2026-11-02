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
}

impl Shown {
    /// The reading and the windows of one probe.
    pub fn of(p: &Probed) -> Self {
        Self {
            reading: p.reading,
            session: p.windows.session.map(|w| w.used),
            week: p.windows.week.map(|w| w.used),
            still_running: p.still_running,
        }
    }

    /// A reading with no windows beyond what it holds itself.
    pub fn from_reading(reading: Reading) -> Self {
        let (session, week) = match reading {
            Reading::Measured { session, week } => (Some(session), Some(week)),
            Reading::Exhausted | Reading::Failed => (None, None),
        };
        Self {
            reading,
            session,
            week,
            still_running: false,
        }
    }
}

/// What the log shows for the live login and each candidate (issue #1844):
/// the windows read when the measurement kept its probes, else the
/// readings alone.
pub fn shown(m: &Measurement) -> (Shown, Vec<(ProfileName, Shown)>) {
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
