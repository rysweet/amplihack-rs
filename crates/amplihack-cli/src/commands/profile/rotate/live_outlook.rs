//! Issue #2211: the figures that stand in for the live reading in the
//! decision, and how they reach it. Pure functions of their arguments: no
//! file, process or clock. The live step (`step` in `live_fresh.rs`) builds
//! the [`Outlook`]; `observe::decide_measured` decides on it under
//! organisation affinity (`observe::affinity::outlook`) and puts that
//! through [`with_outlook`]. See "Rotate live reading freshness" in
//! docs/reference/profile-command.md.

use super::super::super::decide::{Decision, Reading, Reason, needs_swap};
use super::super::super::overflow::Verdict;
use super::super::super::settings::Settings;
use crate::commands::profile::name::ProfileName;

/// `session + rate * interval`, limited to `[0, 100]`.
pub fn projected(session: f64, rate: f64, interval: u64) -> f64 {
    (session + rate * interval as f64).clamp(0.0, 100.0)
}

/// Whether a last known session figure `last` is near `--session-at`:
/// within `--margin` of it, or, with a rate, reaching it one interval ahead.
///
/// A rate only widens the margin band, never narrows it. The rate comes from
/// the series, and when the reading is frozen the series holds the frozen
/// points: identical figures whose observation time keeps moving give a rate
/// of 0, or dilute an earlier rise. Such a rate says nothing about the login,
/// so it must not turn a figure inside the band into "not near".
pub fn near(last: f64, rate: Option<f64>, s: &Settings) -> bool {
    let at = f64::from(s.session_at);
    let by_margin = last >= at - f64::from(s.margin);
    let by_rate = rate.is_some_and(|r| last + r * s.interval as f64 >= at);
    by_margin || by_rate
}

/// What an [`Outlook`]'s figures rest on. With no outlook the decision runs
/// on the reading as measured, so there is no variant for that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
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

impl Outlook {
    /// The outlook's figures as a measured reading, for the decision rule.
    pub fn reading(&self) -> Reading {
        Reading::Measured {
            session: self.session.clamp(0.0, 100.0),
            week: self.week,
        }
    }
}

/// The verdict a cycle acts on when an outlook stands in for the live
/// reading `real_reading`, for a live plan login.
///
/// `real` is the overflow-aware verdict on `real_reading`, as measured,
/// restricted by organisation affinity (#2136). `on_outlook` is the
/// decision on the outlook's figures, made with the cycle's one affinity
/// `Need`, worked out on `real_reading` (`observe::affinity::outlook`, or
/// `outlook_failed` for a failed reading). So the outlook never makes a
/// confirmed organisation crossing, nor makes one sooner.
///
/// - A failed reading: see [`on_failed`]. The failed-probe rules choose the
///   target; the outlook can only remove their damping delay.
/// - A measured reading that is already at a threshold, or that gives a
///   swap on its own, decides by itself: `real` stands, as without the
///   outlook (its threshold, plan-room, overflow or upgrade swap, or its
///   ALERT).
/// - Otherwise the measured reading is below both thresholds, so `real` is
///   a stay, and the outlook decides: a threshold swap on it, labelled by
///   [`map_reason`], or the `no-capacity` ALERT. An ALERT is a stay with a
///   warning, so no swap is lost. A stay on the outlook, which affinity
///   gives when only a confirmed crossing has capacity, leaves `real`.
///
/// So the plan-room swap and the moves onto overflow never run on a
/// projected or substituted figure, and nothing here moves work onto
/// credits or the API key.
pub fn with_outlook(
    real: Verdict,
    on_outlook: Decision,
    basis: Basis,
    s: &Settings,
    real_reading: &Reading,
) -> Verdict {
    if *real_reading == Reading::Failed {
        return on_failed(real, &on_outlook, basis, s, real_reading);
    }
    if needs_swap(s, real_reading) || matches!(real.decision, Decision::Swap { .. }) {
        return real;
    }
    let decision = match on_outlook {
        d @ Decision::Swap {
            reason: Reason::Threshold,
            ..
        } => map_reason(d, basis, s, real_reading),
        Decision::Alert => Decision::Alert,
        _ => return real,
    };
    Verdict {
        decision,
        tag: None,
        next_plan_reset: None,
        key_held: Vec::new(),
        ..real
    }
}

/// The verdict on a failed live reading under a substitution. The outlook
/// changes the timing only, never the choice of target.
///
/// For a failed live plan login, `real` is the decision rule on headroom 0,
/// restricted by organisation affinity: a threshold swap to the permitted
/// candidate with the most headroom, when it has `--margin` of headroom,
/// which failed-probe damping delays for three failures, or the ALERT.
///
/// `on_outlook` asks one thing of `real`'s own target: whether it has
/// `--margin` more headroom than the substituted figure leaves the live
/// login (`observe::affinity::outlook_failed`, which also refuses a target
/// that is a confirmed organisation crossing). Only a threshold swap to
/// that same target counts here; a swap the outlook names to any other
/// login is ignored, since another login beating the substituted figure
/// says nothing about `real`'s target.
///
/// - The outlook gives a threshold swap to `real`'s target: that target
///   beats even the substituted figure, so `real`'s swap is made now,
///   relabelled by [`map_reason`] so that damping does not delay it.
/// - Otherwise `real` stands, so damping and the `failed-probe` swap after
///   three failures apply as without the outlook. An ALERT on the outlook
///   never replaces that swap: a dead live login must still be left.
///
/// The result is either `real` or `real` made earlier.
fn on_failed(
    real: Verdict,
    on_outlook: &Decision,
    basis: Basis,
    s: &Settings,
    real_reading: &Reading,
) -> Verdict {
    let sooner = match (&real.decision, on_outlook) {
        (
            Decision::Swap { to, .. },
            Decision::Swap {
                to: target,
                reason: Reason::Threshold,
            },
        ) => to == target,
        _ => false,
    };
    if !sooner {
        return real;
    }
    Verdict {
        decision: map_reason(real.decision.clone(), basis, s, real_reading),
        ..real
    }
}

/// Relabels a threshold swap made on an outlook as `projected` only when
/// the `real` reading would not itself have needed a swap; a failed real
/// reading under a substitution counts as not needing one. Never creates,
/// removes or retargets a swap.
pub fn map_reason(d: Decision, basis: Basis, s: &Settings, real: &Reading) -> Decision {
    let relabel = match basis {
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
