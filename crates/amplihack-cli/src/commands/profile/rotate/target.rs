//! The one pure rule for choosing a swap target (issue #2910): room now,
//! eligibility, the room a move needs, the ranking, the hold on moves past
//! `--session-at`, the swap reason and why a cycle did not move. It reads no
//! clock and does no I/O; `decide` is built on it, and every pass (threshold,
//! plan room, upgrade, failed probe, projected and overflow-out) ranks with
//! [`ranked`]. See "Rotate target choice" in docs/reference/profile-command.md.
//!
//! - Eligible ([`fit`]): no spend limit, a usable reading, both windows below
//!   100%, and a week at or below `--target-week-max`. A recorded block is
//!   applied by the callers, which give a blocked login a reading with no room.
//! - Qualifying ([`relief_floor`], [`upgrade_floor`]): a live login with no
//!   room now takes any eligible login; one with room needs a login with
//!   `max(--margin, 1)` more room now; below both thresholds the upgrade
//!   rule applies.
//! - Ranked ([`order`]): session below `--session-at` first, then the larger
//!   room now, then the lower week, then the name in byte order.

use super::super::settings::Settings;
use super::{Measured, Reading, Reason, needs_swap};
use crate::commands::profile::name::ProfileName;
use std::cmp::Ordering;

/// Points of room a login can use now, `floor(100 - max(session, week))`,
/// when both windows are from 0 up to below 100; `None` otherwise (a full
/// window, or a figure no probe produces: negative, NaN or infinite).
pub fn room_now(session: f64, week: f64) -> Option<u8> {
    let open = |v: f64| (0.0..100.0).contains(&v);
    if !(open(session) && open(week)) {
        return None;
    }
    // Both are in 0..100, so the result is in 0..=100 and the cast is exact.
    Some((100.0 - session.max(week)).floor() as u8)
}

/// Why a login is not eligible, in the order [`fit`] checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unfit {
    /// Claude reported the organisation's spend limit for it.
    SpendLimit,
    /// No usable figures: a failed probe, or a figure that is negative, NaN
    /// or infinite.
    NoReading,
    /// A window at 100% or more.
    Full,
    /// Its week is above `--target-week-max`.
    AboveCeiling,
}

/// A figure that no probe produces: never ranked (S3).
fn unusable(v: f64) -> bool {
    !v.is_finite() || v < 0.0
}

/// The room now of an eligible login, or why it is not eligible (F1). The
/// ceiling is inclusive: a week of exactly `week_max` is eligible.
pub fn fit(m: &Measured, week_max: u8) -> Result<u8, Unfit> {
    if m.spend_limit {
        return Err(Unfit::SpendLimit);
    }
    let (session, week) = match m.reading {
        Reading::Failed => return Err(Unfit::NoReading),
        Reading::Exhausted { .. } => return Err(Unfit::Full),
        Reading::Measured { session, week } => (session, week),
    };
    if unusable(session) || unusable(week) {
        return Err(Unfit::NoReading);
    }
    if session >= 100.0 || week >= 100.0 {
        return Err(Unfit::Full);
    }
    if week > f64::from(week_max) {
        return Err(Unfit::AboveCeiling);
    }
    room_now(session, week).ok_or(Unfit::NoReading)
}

/// The live login's room now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveRoom {
    /// A window at 100%, or an active recorded block (E11).
    NoRoom,
    /// The probe read nothing usable.
    Failed,
    /// Points of room now.
    Room(u8),
}

/// The live login's [`LiveRoom`]; `blocked` is an active recorded block on it.
pub fn live_room(live: &Reading, blocked: bool) -> LiveRoom {
    if blocked {
        return LiveRoom::NoRoom;
    }
    match *live {
        Reading::Failed => LiveRoom::Failed,
        Reading::Exhausted { .. } => LiveRoom::NoRoom,
        Reading::Measured { session, week } => match room_now(session, week) {
            Some(r) => LiveRoom::Room(r),
            None if unusable(session) || unusable(week) => LiveRoom::Failed,
            None => LiveRoom::NoRoom,
        },
    }
}

/// The least room now a login needs to relieve a live login past a
/// threshold (F2): nothing with no room (D2), the margin after a failed
/// probe, and the live room plus `max(margin, 1)` otherwise (D1).
pub fn relief_floor(live: LiveRoom, margin: u8) -> u16 {
    match live {
        LiveRoom::NoRoom => 0,
        LiveRoom::Failed => u16::from(margin),
        LiveRoom::Room(r) => u16::from(r) + u16::from(margin.max(1)),
    }
}

/// The least room now an upgrade target needs: more than the live room, and
/// at least `upgrade_margin` more.
pub fn upgrade_floor(live_room: u8, upgrade_margin: u8) -> u16 {
    u16::from(live_room) + u16::from(upgrade_margin.max(1))
}

/// The session use of `m`, for the tier; `None` when it has no figures.
fn session_of(m: &Measured) -> Option<f64> {
    match m.reading {
        Reading::Measured { session, .. } => Some(session),
        Reading::Exhausted { .. } | Reading::Failed => None,
    }
}

/// Whether `m` is at or above `--session-at`: the second tier of [`order`],
/// and what the hold of D3 applies to. A login with no figures counts as
/// past it, which [`fit`] makes unreachable for a ranked login.
fn past_at(m: &Measured, session_at: u8) -> bool {
    session_of(m).is_none_or(|v| v >= f64::from(session_at))
}

/// Whether the login `name` in `candidates` is [`past_at`] `--session-at`.
/// A name the list does not hold counts as past it; [`ranked`] names only
/// logins from the list it was given, so that too is unreachable.
pub fn past_session_at(s: &Settings, candidates: &[Measured], name: &ProfileName) -> bool {
    candidates
        .iter()
        .find(|c| &c.name == name)
        .is_none_or(|c| past_at(c, s.session_at))
}

fn week_of(m: &Measured) -> f64 {
    match m.reading {
        Reading::Measured { week, .. } => week,
        Reading::Exhausted { .. } | Reading::Failed => f64::INFINITY,
    }
}

/// The ranking of two eligible logins with room now `ra` and `rb` (F3):
/// session below `--session-at` first, then more room, then the lower week,
/// then the name in ascending byte order.
pub fn order(s: &Settings, a: (&Measured, u8), b: (&Measured, u8)) -> Ordering {
    let tier = |m: &Measured| past_at(m, s.session_at);
    tier(a.0)
        .cmp(&tier(b.0))
        .then_with(|| b.1.cmp(&a.1))
        .then_with(|| week_of(a.0).total_cmp(&week_of(b.0)))
        .then_with(|| {
            a.0.name
                .as_str()
                .as_bytes()
                .cmp(b.0.name.as_str().as_bytes())
        })
}

/// The eligible logins with at least `floor` room now, best first, with
/// their room now.
pub fn ranked<'a>(
    s: &Settings,
    candidates: &'a [Measured],
    floor: u16,
) -> Vec<(&'a ProfileName, u8)> {
    let mut fit: Vec<(&Measured, u8)> = candidates
        .iter()
        .filter_map(|c| fit(c, s.target_week_max).ok().map(|r| (c, r)))
        .filter(|(_, r)| u16::from(*r) >= floor)
        .collect();
    fit.sort_by(|a, b| order(s, *a, *b));
    fit.into_iter().map(|(c, r)| (&c.name, r)).collect()
}

/// D3: whether a move to a login at or above `--session-at` away from a live
/// login that still has room waits, because the rotator's own swap at
/// `swapped_at` was less than `hold` seconds ago. `hold` 0 disables it, a hand
/// switch leaves no `swapped_at`, and a swap time in the future (a clock
/// change) is no evidence of a recent swap, so it never holds (S11).
pub fn held_move(hold: u64, swapped_at: Option<i64>, now: i64) -> bool {
    swapped_at.is_some_and(|t| t <= now) && super::held(hold, swapped_at, now)
}

/// D15: `threshold` for a target below both thresholds, `plan-room` for one
/// with room that is at or above one of them.
pub fn swap_reason(s: &Settings, to: &Reading) -> Reason {
    match *to {
        Reading::Measured { session, week }
            if session < f64::from(s.session_at) && week < f64::from(s.week_at) =>
        {
            Reason::Threshold
        }
        _ => Reason::PlanRoom,
    }
}

/// Why a cycle that had to consider a move did not make one.
#[derive(Debug, Clone, PartialEq)]
pub enum NoMove {
    /// No login is eligible.
    NoTarget,
    /// Logins are eligible, but none has the room a move needs (D1). `most`
    /// is the most room of a login the rotator may move to, `None` when only
    /// a login in another organisation has room (S1). `room` is the live
    /// login's room, `None` when its probe read nothing usable. `needs` is
    /// what a move needs.
    LessRoom {
        most: Option<u8>,
        room: Option<u8>,
        needs: u16,
    },
    /// The best qualifying login is at or above `--session-at`, and the hold
    /// after the rotator's own swap `since` seconds ago applies (D3).
    Held {
        to: ProfileName,
        session: f64,
        since: u64,
    },
    /// Every qualifying login was rejected at confirmation, and one of them
    /// is still eligible because its re-read was unknown (D7).
    Unconfirmed,
}

/// Why `decide` made no move on these readings. `candidates` are the logins
/// the rotator may move to (plan logins, with a blocked one carrying no
/// room), so a login another organisation holds is left out by the caller.
pub fn no_move(
    s: &Settings,
    live: &Reading,
    candidates: &[Measured],
    swapped_at: Option<i64>,
    now: i64,
) -> NoMove {
    let lr = live_room(live, false);
    let Some(most) = candidates
        .iter()
        .filter_map(|c| fit(c, s.target_week_max).ok())
        .max()
    else {
        return NoMove::NoTarget;
    };
    let room = match lr {
        LiveRoom::Room(r) => Some(r),
        LiveRoom::NoRoom => Some(0),
        LiveRoom::Failed => None,
    };
    let needs = match lr {
        LiveRoom::Room(r) if !needs_swap(s, live) => upgrade_floor(r, s.upgrade_margin),
        _ => relief_floor(lr, s.margin),
    };
    let less = NoMove::LessRoom {
        most: Some(most),
        room,
        needs,
    };
    let Some((to, _)) = ranked(s, candidates, needs).into_iter().next() else {
        return less;
    };
    // The hold of `decide`, with the target's own session figure, which a
    // login `ranked` names always has: `fit` accepts only a measured reading.
    if matches!(lr, LiveRoom::Room(_))
        && held_move(s.hold, swapped_at, now)
        && past_session_at(s, candidates, to)
        && let Some(session) = candidates
            .iter()
            .find(|c| &c.name == to)
            .and_then(session_of)
    {
        let since = swapped_at.map_or(0, |t| u64::try_from(now.saturating_sub(t)).unwrap_or(0));
        return NoMove::Held {
            to: to.clone(),
            session,
            since,
        };
    }
    less
}

#[cfg(test)]
#[path = "tests_issue_2910_rule.rs"]
mod tests_issue_2910_rule;
