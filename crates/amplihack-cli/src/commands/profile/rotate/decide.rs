//! The decision rule (D1-D5): a pure function of the settings, the live
//! reading, the candidates' readings, the last swap time and the clock.

use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::usage_parse::{UsageReport, UsageState, UsageWindow};

/// What one probe showed, reduced to what the rule needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    /// Percent used in the session and weekly (all models) windows.
    Measured { session: f64, week: f64 },
    /// The session or weekly limit is reached.
    Exhausted,
    /// The probe failed or its output was incomplete.
    Failed,
}

impl Reading {
    /// D1: a missing session window is 0% used; a missing week window, or
    /// any state other than ok or exhausted, is a failed probe.
    pub fn from_report(r: &UsageReport) -> Self {
        match r.state {
            UsageState::SessionExhausted | UsageState::WeeklyExhausted => Self::Exhausted,
            UsageState::Ok => match &r.week_all {
                Some(week) => Self::Measured {
                    session: r.session.as_ref().map_or(0.0, |w| w.used_percentage),
                    week: week.used_percentage,
                },
                None => Self::Failed,
            },
            _ => Self::Failed,
        }
    }

    /// Points left in the worse window, rounded down and clamped to 0-100.
    /// `None` for a failed probe.
    pub fn headroom(&self) -> Option<u8> {
        match self {
            Self::Failed => None,
            Self::Exhausted => Some(0),
            Self::Measured { session, week } => {
                let left = (100.0 - session.max(*week)).floor();
                if left.is_nan() {
                    return None;
                }
                Some(left.clamp(0.0, 100.0) as u8)
            }
        }
    }
}

/// One usage window as measured: percent used and when it resets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Win {
    pub used: f64,
    /// Epoch seconds; `None` when Claude printed no readable reset.
    pub resets_at: Option<i64>,
}

/// The session and weekly (all models) windows of one probe, kept for every
/// state, exhausted ones included, so the overflow rule knows when a plan
/// login is back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Windows {
    pub session: Option<Win>,
    pub week: Option<Win>,
}

impl Windows {
    pub fn from_report(r: &UsageReport) -> Self {
        let win = |w: &UsageWindow| Win {
            used: w.used_percentage,
            resets_at: w.resets_at,
        };
        Self {
            session: r.session.as_ref().map(win),
            week: r.week_all.as_ref().map(win),
        }
    }
}

/// A probe as the rotator keeps it: the reading `decide` uses, the windows
/// with their reset times, and whether Claude reported the organisation's
/// spend limit (which still reads `Failed`).
#[derive(Debug, Clone, PartialEq)]
pub struct Probed {
    pub reading: Reading,
    pub windows: Windows,
    pub spend_limit: bool,
    /// The probe timed out or could not run: the login may have capacity,
    /// so it blocks a move onto overflow. Always with `Reading::Failed`.
    pub unknown: bool,
    /// Issue #1844: the read had not finished when the cycle stopped waiting
    /// for it. Read only by the log (`log::Shown`); always with `unknown`.
    pub still_running: bool,
}

impl Probed {
    /// A `timeout` or `error` report is unknown; every other state is what
    /// the login reported.
    pub fn from_report(r: &UsageReport) -> Self {
        Self {
            reading: Reading::from_report(r),
            windows: Windows::from_report(r),
            spend_limit: r.state == UsageState::SpendLimit,
            unknown: matches!(r.state, UsageState::Timeout | UsageState::Error),
            still_running: false,
        }
    }

    /// The synthetic reading of an API-key profile (issue #1700), which is
    /// never probed: usable, 0% of no plan windows, never a spend limit. It
    /// is never a plan reading: an API-key profile is always in the overflow
    /// order, so its 0/0 never counts as plan capacity.
    pub fn platform() -> Self {
        Self {
            reading: Reading::Measured {
                session: 0.0,
                week: 0.0,
            },
            windows: Windows::default(),
            spend_limit: false,
            unknown: false,
            still_running: false,
        }
    }

    /// A probe that failed before any output was read: unknown.
    pub fn failed() -> Self {
        Self {
            reading: Reading::Failed,
            windows: Windows::default(),
            spend_limit: false,
            unknown: true,
            still_running: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub name: ProfileName,
    pub reading: Reading,
    pub windows: Windows,
    /// Claude reported the organisation's spend limit for this profile.
    pub spend_limit: bool,
    /// As [`Probed::unknown`].
    pub unknown: bool,
}

impl Measured {
    pub fn new(name: ProfileName, p: &Probed) -> Self {
        Self {
            name,
            reading: p.reading,
            windows: p.windows.clone(),
            spend_limit: p.spend_limit,
            unknown: p.unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Threshold,
    Upgrade,
    /// A threshold swap made after repeated failed live probes; see `damp`.
    FailedProbe,
    /// A move onto a credit-backed overflow profile; see `overflow`.
    OverflowIn,
    /// A move off an overflow profile onto a plan login with capacity.
    OverflowOut,
    /// A move to the plan login with the most room when no threshold swap
    /// meets `--margin` (issue #1804); see `plan_room`.
    PlanRoom,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Threshold => "threshold",
            Self::Upgrade => "upgrade",
            Self::FailedProbe => "failed-probe",
            Self::OverflowIn => "overflow-in",
            Self::OverflowOut => "overflow-out",
            Self::PlanRoom => "plan-room",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Stay,
    Swap {
        to: ProfileName,
        reason: Reason,
    },
    /// A threshold is reached and no candidate has enough headroom.
    Alert,
}

/// Whether the live login has reached a threshold. A failed probe counts.
pub(super) fn needs_swap(s: &Settings, live: &Reading) -> bool {
    match live {
        Reading::Failed | Reading::Exhausted => true,
        Reading::Measured { session, week } => {
            *session >= f64::from(s.session_at) || *week >= f64::from(s.week_at)
        }
    }
}

/// The candidate with the most headroom; ties go to the first name. Failed
/// and exhausted candidates are never chosen.
pub(super) fn best(candidates: &[Measured]) -> Option<(&ProfileName, u8)> {
    candidates
        .iter()
        .filter(|c| !matches!(c.reading, Reading::Exhausted))
        .filter_map(|c| c.reading.headroom().map(|h| (&c.name, h)))
        .min_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.as_str().cmp(b.0.as_str())))
}

pub fn decide(
    s: &Settings,
    live: &Reading,
    candidates: &[Measured],
    swapped_at: Option<i64>,
    now: i64,
) -> Decision {
    let live_h = i32::from(live.headroom().unwrap_or(0));
    let best = best(candidates);
    let gain = best.map(|(_, h)| i32::from(h) - live_h);
    if needs_swap(s, live) {
        return match (best, gain) {
            (Some((to, _)), Some(g)) if g >= i32::from(s.margin) => Decision::Swap {
                to: to.clone(),
                reason: Reason::Threshold,
            },
            _ => Decision::Alert,
        };
    }
    let held = held(s.hold, swapped_at, now);
    match (best, gain) {
        (Some((to, _)), Some(g)) if !held && g > 0 && g >= i32::from(s.upgrade_margin) => {
            Decision::Swap {
                to: to.clone(),
                reason: Reason::Upgrade,
            }
        }
        _ => Decision::Stay,
    }
}

/// Whether a swap at `swapped_at` is less than `hold` seconds ago; `hold` 0
/// disables it. A swap time in the future (a clock change) still holds.
pub(super) fn held(hold: u64, swapped_at: Option<i64>, now: i64) -> bool {
    hold > 0
        && swapped_at
            .is_some_and(|t| now.saturating_sub(t) < i64::try_from(hold).unwrap_or(i64::MAX))
}
