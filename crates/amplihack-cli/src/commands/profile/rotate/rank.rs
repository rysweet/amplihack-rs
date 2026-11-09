//! Issue #2013 (D3, D4, D16, D17): the one rule for which logins can be
//! chosen and in what order. The threshold and upgrade swaps
//! (`decide::decide`), the plan-room swap (`plan_room::choose`) and the
//! overflow policy all rank with it, so no two paths disagree on the best
//! login. Pure functions, no side effects. See "Rotate eligible logins and
//! ranking" in docs/reference/profile-command.md.
//!
//! Freshness is applied before these rules, by the caller: a reading that
//! is not fresh never reaches them as a measured one.

use super::super::decide::{Measured, Reading};
use super::super::plan_room::has_room;
use super::super::settings::Settings;
use std::cmp::Ordering;

/// Whether a login reading `r` can be chosen: a measured reading with
/// session use below `--session-at` and week use below `--week-at`. A
/// failed, exhausted or unreadable (NaN) reading never is.
pub fn eligible(s: &Settings, r: &Reading) -> bool {
    match r {
        Reading::Measured { session, week } => {
            *session < f64::from(s.session_at) && *week < f64::from(s.week_at)
        }
        Reading::Exhausted { .. } | Reading::Failed => false,
    }
}

/// The ranking key of `r`: week use, then session use. A reading that is
/// not measured ranks after every measured one.
fn key(r: &Reading) -> (f64, f64) {
    match r {
        Reading::Measured { session, week } => (*week, *session),
        Reading::Exhausted { .. } | Reading::Failed => (f64::INFINITY, f64::INFINITY),
    }
}

/// The rank order: week use ascending, then session use ascending, then
/// name. Weekly exhaustion lasts longer than session exhaustion, so the
/// week decides first.
pub fn order(a: &Measured, b: &Measured) -> Ordering {
    let (aw, as_) = key(&a.reading);
    let (bw, bs) = key(&b.reading);
    aw.total_cmp(&bw)
        .then_with(|| as_.total_cmp(&bs))
        .then_with(|| a.name.as_str().cmp(b.name.as_str()))
}

/// The eligible candidates in rank order.
pub fn ranked_eligible<'a>(s: &Settings, candidates: &'a [Measured]) -> Vec<&'a Measured> {
    let mut out: Vec<&Measured> = candidates
        .iter()
        .filter(|c| eligible(s, &c.reading))
        .collect();
    out.sort_by(|a, b| order(a, b));
    out
}

/// The first eligible candidate in rank order.
pub fn best_eligible<'a>(s: &Settings, candidates: &'a [Measured]) -> Option<&'a Measured> {
    candidates
        .iter()
        .filter(|c| eligible(s, &c.reading))
        .min_by(|a, b| order(a, b))
}

/// Points of weekly room `r` has left, `100 - week`, clamped to 0-100. A
/// failed, exhausted or unreadable reading has none.
pub fn weekly_headroom(r: &Reading) -> f64 {
    match r {
        Reading::Measured { week, .. } if week.is_finite() => (100.0 - week).clamp(0.0, 100.0),
        _ => 0.0,
    }
}

/// Whether `to` beats a login reading `live` by at least `--margin` points
/// of weekly headroom. A live login without room (failed, exhausted or at
/// 100% in a window) cannot be worked on, so any reading beats it.
pub fn beats_by_margin(s: &Settings, live: &Reading, to: &Reading) -> bool {
    !has_room(live) || weekly_headroom(to) - weekly_headroom(live) >= f64::from(s.margin)
}

/// D4: when no candidate is eligible, the first candidate with room (both
/// windows below 100%) in rank order whose weekly headroom beats the live
/// login's by `--margin`. Rank order puts the most weekly headroom first,
/// so if the first candidate with room does not beat the live login, none
/// does.
pub fn best_with_margin<'a>(
    s: &Settings,
    live: &Reading,
    candidates: &'a [Measured],
) -> Option<&'a Measured> {
    candidates
        .iter()
        .filter(|c| has_room(&c.reading))
        .min_by(|a, b| order(a, b))
        .filter(|c| beats_by_margin(s, live, &c.reading))
}

/// The first measured candidate in rank order, room or not: the last
/// resort off a blocked API key or overflow profile.
pub fn best_measured(candidates: &[Measured]) -> Option<&Measured> {
    candidates
        .iter()
        .filter(|c| match c.reading {
            Reading::Measured { session, week } => !session.is_nan() && !week.is_nan(),
            Reading::Exhausted { .. } | Reading::Failed => false,
        })
        .min_by(|a, b| order(a, b))
}
