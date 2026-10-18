//! The decision rule (D1-D5): a pure function of the settings, the live
//! reading, the candidates' readings, the last swap time and the clock.

use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::usage_parse::{UsageReport, UsageState};

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

#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub name: ProfileName,
    pub reading: Reading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Threshold,
    Upgrade,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Threshold => "threshold",
            Self::Upgrade => "upgrade",
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
fn needs_swap(s: &Settings, live: &Reading) -> bool {
    match live {
        Reading::Failed | Reading::Exhausted => true,
        Reading::Measured { session, week } => {
            *session >= f64::from(s.session_at) || *week >= f64::from(s.week_at)
        }
    }
}

/// The candidate with the most headroom; ties go to the first name. Failed
/// and exhausted candidates are never chosen.
fn best(candidates: &[Measured]) -> Option<(&ProfileName, u8)> {
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
    // A swap time in the future (a clock change) still holds.
    let held = s.hold > 0
        && swapped_at
            .is_some_and(|t| now.saturating_sub(t) < i64::try_from(s.hold).unwrap_or(i64::MAX));
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
