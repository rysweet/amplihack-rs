//! The decision rule (D1-D5): a pure function of the settings, the live
//! reading, the candidates' readings, the last swap time and the clock. Issue
//! #2910: the choice of target is the one rule in `target`.

use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::usage_parse::{UsageReport, UsageState, UsageWindow};

// Issue #2910: the one rule for choosing a swap target, and what it needs.
#[path = "last_good.rs"]
pub(super) mod last_good;
#[path = "target.rs"]
pub(super) mod target;
#[path = "target_confirm.rs"]
pub(super) mod target_confirm;
#[path = "target_msg.rs"]
pub(super) mod target_msg;
#[path = "target_seen.rs"]
pub(super) mod target_seen;
#[path = "weekly.rs"]
mod weekly;

/// What one probe showed, reduced to what the rule needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    /// Percent used in the session window, and the largest percent used of
    /// the weekly windows (all models and each model, issue #3284).
    Measured { session: f64, week: f64 },
    /// The session or weekly limit is reached. The fields keep the percent
    /// used as read: the window that ran out is never below 100, the other
    /// is as measured, and `None` means the report had no such window.
    /// They are for display only; decisions read `Windows`, not these fields.
    Exhausted {
        session: Option<f64>,
        week: Option<f64>,
    },
    /// The probe failed or its output was incomplete.
    Failed,
}

impl Reading {
    /// D1: a missing session window is 0% used; a missing week window, or
    /// any state other than ok or exhausted, is a failed probe. An exhausted
    /// report keeps both figures, with the window that ran out raised to 100.
    ///
    /// Issue #3284: the week figure is the largest of the all-models week and
    /// every per-model week, and a model week at 100 or more is an exhausted
    /// week, so a login whose week for one model is used up has no room.
    pub fn from_report(r: &UsageReport) -> Self {
        let used = |w: &Option<UsageWindow>| w.as_ref().map(|w| w.used_percentage);
        let models = r.week_models.iter().map(|w| w.used_percentage);
        let windows = Windows::from_report(r);
        let week = windows.deciding_week().map(|w| w.used);
        let model_full = models.clone().any(|u| u >= 100.0);
        let full =
            |w: Option<f64>| Some(w.filter(|u| u.is_finite()).map_or(100.0, |u| u.max(100.0)));
        match r.state {
            UsageState::SessionExhausted => Self::Exhausted {
                session: full(used(&r.session)),
                week,
            },
            UsageState::WeeklyExhausted => Self::Exhausted {
                session: used(&r.session),
                week: full(week),
            },
            UsageState::Ok => match week {
                Some(week) if model_full => Self::Exhausted {
                    session: used(&r.session),
                    week: full(Some(week)),
                },
                Some(week) => Self::Measured {
                    session: r.session.as_ref().map_or(0.0, |w| w.used_percentage),
                    week,
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
            Self::Exhausted { .. } => Some(0),
            Self::Measured { session, week } => {
                let left = (100.0 - session.max(*week)).floor();
                if left.is_nan() {
                    return None;
                }
                Some(left.clamp(0.0, 100.0) as u8)
            }
        }
    }

    /// An exhausted reading with both windows at 100%: the meaning the
    /// variant had before it kept its figures.
    #[cfg(test)]
    pub const EXHAUSTED: Self = Self::Exhausted {
        session: Some(100.0),
        week: Some(100.0),
    };
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
    /// Individual model observations, separate from the all-models reset.
    pub week_models: Vec<UsageWindow>,
    /// Issue #1818: the reading showed no session window, so no session
    /// limit applies (`UsageReport::session_limit` is `false`). `false` for
    /// a session that was read and for a probe that read nothing, so a
    /// missing reading is never taken as room.
    pub no_session_limit: bool,
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
            week_models: r.week_models.clone(),
            no_session_limit: !r.session_limit,
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
    /// so it blocks a move onto overflow. With `Reading::Failed`, except for
    /// [`Probed::excluded`], whose login is never read (issue #2218).
    pub unknown: bool,
    /// Issue #1844: the read had not finished when the cycle stopped waiting
    /// for it. Read only by the log (`log::Shown`); always with `unknown`.
    pub still_running: bool,
    /// Issue #1797: when a passive reading was taken, if before the read
    /// (`UsageReport::observed_at`), when the #1870 re-read finished
    /// (`fresh::fresh_probe`), or, issue #2909, when an earlier cycle took a
    /// stored reading (`cycle::event_read::Stored::usable`); `None` for every
    /// other probe.
    pub observed_at: Option<i64>,
    /// Issue #1870: the result of a read started in an earlier cycle, so it
    /// has no reading time (`fresh::read_at`). Issue #2909: also a reading an
    /// earlier cycle took, which an event cycle decides on in place of a read
    /// (`cycle::event_read`); the freshness gate reads it again before a swap.
    pub carried: bool,
    /// Issue #1970: the figure Claude's status line stored (a passive
    /// reading, `usage_passive`), not one read by this cycle. Set from the
    /// source in [`Probed::from_report`]; `observed_at` cannot stand in for
    /// it, because a #1870 re-read is a direct read that also carries one.
    pub passive: bool,
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
            observed_at: r.observed_at,
            carried: false,
            // `UsageReport::observed_at` is set only by the passive reader.
            passive: r.observed_at.is_some(),
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
            observed_at: None,
            carried: false,
            passive: false,
        }
    }

    /// Issue #2218: the live login of a profile in the auto-switch `exclude`
    /// list, which is never probed. It reads as exhausted with no figures,
    /// so the first cycle must swap: not `failed`, which damping would hold
    /// for several cycles, nor `platform`, which reads as room. It is
    /// unknown, so it is never counted as fresh room or a measured login.
    pub fn excluded() -> Self {
        Self {
            reading: Reading::Exhausted {
                session: None,
                week: None,
            },
            windows: Windows::default(),
            spend_limit: false,
            unknown: true,
            still_running: false,
            observed_at: None,
            carried: false,
            passive: false,
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
            observed_at: None,
            carried: false,
            passive: false,
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
    /// Issue #2910 (D15): a relief move onto a plan login that has room but
    /// is at or above `--session-at` or `--week-at`; see `target::swap_reason`.
    PlanRoom,
    /// Issue #2211: a threshold swap decided on the live login's projected
    /// session use, or on a failed or stale reading near the threshold; see
    /// `fresh::live` (`live_fresh.rs`).
    Projected,
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
            Self::Projected => "projected",
        }
    }
    /// Issue #3181: a swap the live login needs, because it is at or near
    /// its limit or its reading near the threshold cannot be trusted. Such a
    /// swap is not refused for the identity read budget.
    pub fn live_needs_it(self) -> bool {
        match self {
            Self::Threshold | Self::FailedProbe | Self::Projected => true,
            Self::Upgrade | Self::OverflowIn | Self::OverflowOut | Self::PlanRoom => false,
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
    /// A threshold is reached and the rotator does not move: no candidate
    /// qualifies, or the best one is held (D3). `target::no_move` says which,
    /// and the cycle writes a stay note or the no-target line accordingly.
    Alert,
}

/// Whether the live login has reached a threshold. A failed probe counts.
pub(super) fn needs_swap(s: &Settings, live: &Reading) -> bool {
    match live {
        Reading::Failed | Reading::Exhausted { .. } => true,
        Reading::Measured { session, week } => {
            *session >= f64::from(s.session_at) || *week >= f64::from(s.week_at)
        }
    }
}

/// The decision rule on [`target`] (issue #2910): one eligibility test, one
/// qualifying rule and one ranking for every pass.
///
/// - Past a threshold (or failed, or with no room): a threshold swap to the
///   top of [`target::ranked`] at [`target::relief_floor`]. A swap that the
///   hold stops (D3: the live login has room, the rotator swapped less than
///   `--hold` ago, and the target is at or above `--session-at`) is the
///   ALERT, as is no qualifying login; [`target::no_move`] says which. The
///   reason is always `threshold` here: the cycle relabels a target past a
///   threshold as `plan-room` afterwards ([`target::swap_reason`]), so the
///   failed-probe and projected rules still see a threshold swap.
/// - Below both thresholds: an upgrade to the top of the ranking at
///   [`target::upgrade_floor`] once the hold has run out, else a stay.
pub fn decide(
    s: &Settings,
    live: &Reading,
    candidates: &[Measured],
    swapped_at: Option<i64>,
    now: i64,
) -> Decision {
    let lr = target::live_room(live, false);
    if needs_swap(s, live) {
        let ranked = target::ranked(s, candidates, target::relief_floor(lr, s.margin));
        let Some((to, _)) = ranked.first() else {
            return Decision::Alert;
        };
        if matches!(lr, target::LiveRoom::Room(_))
            && target::past_session_at(s, candidates, to)
            && target::held_move(s.hold, swapped_at, now)
        {
            return Decision::Alert;
        }
        return Decision::Swap {
            to: (*to).clone(),
            reason: Reason::Threshold,
        };
    }
    let target::LiveRoom::Room(room) = lr else {
        return Decision::Stay;
    };
    if held(s.hold, swapped_at, now) {
        return Decision::Stay;
    }
    match target::ranked(s, candidates, target::upgrade_floor(room, s.upgrade_margin)).first() {
        Some((to, _)) => Decision::Swap {
            to: (*to).clone(),
            reason: Reason::Upgrade,
        },
        None => Decision::Stay,
    }
}

/// Whether a swap at `swapped_at` is less than `hold` seconds ago; `hold` 0
/// disables it. A swap time in the future (a clock change) still holds.
pub(super) fn held(hold: u64, swapped_at: Option<i64>, now: i64) -> bool {
    hold > 0
        && swapped_at
            .is_some_and(|t| now.saturating_sub(t) < i64::try_from(hold).unwrap_or(i64::MAX))
}
