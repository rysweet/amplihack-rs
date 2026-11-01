//! The plan-room swap (issue #1804): when no threshold swap meets
//! `--margin`, move to the plan login with the most weekly room among
//! those with session room, rather than raise an ALERT or spend money. Pure
//! functions; see `tests_issue_1804` and "Rotate plan-room swap" in
//! docs/reference/profile-command.md.
//!
//! "Room" is weaker than capacity: a login has room while both of its
//! windows are below 100%, whatever `--session-at` and `--week-at` say.
//! Only a measured reading has room, so a login that failed its probe or
//! was never measured is not a move target for room. That does not make it
//! a login without room: its state is unknown, and `overflow::decide_overflow`
//! keeps the API key out while any plan login is unknown. The rotator and
//! auto-switch both use [`room`].

use super::decide::{Measured, Reading};
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use std::cmp::Ordering;

/// Whether a plan login using `session`% and `week`% has room: both below
/// 100. An unreadable value (NaN) is no room.
pub fn room(session: f64, week: f64) -> bool {
    session < 100.0 && week < 100.0
}

/// Whether `r` is a measured reading with room.
pub fn has_room(r: &Reading) -> bool {
    match r {
        Reading::Measured { session, week } => room(*session, *week),
        Reading::Exhausted | Reading::Failed => false,
    }
}

/// The plan login to move to for room, if any. Pass 1 takes the logins with
/// session below `--session-at` and week below 100; pass 2, only when pass
/// 1 is empty (the live login included), takes those with room. Each pass
/// ranks by week, then session, then name. `None` when nothing qualifies
/// or when the live login ranks first, so the rotator does not swap back
/// and forth between two logins with the same week.
///
/// `plan` must hold plan logins only: the caller leaves every overflow
/// profile out, so an API-key profile is never chosen.
pub fn choose(s: &Settings, live: Option<&Measured>, plan: &[Measured]) -> Option<ProfileName> {
    let session_at = f64::from(s.session_at);
    let pool: Vec<(&ProfileName, f64, f64)> = live
        .into_iter()
        .chain(plan)
        .filter_map(|m| match m.reading {
            Reading::Measured { session, week } if room(session, week) => {
                Some((&m.name, session, week))
            }
            _ => None,
        })
        .collect();
    let first_pass: Vec<_> = pool
        .iter()
        .copied()
        .filter(|(_, session, _)| *session < session_at)
        .collect();
    let pass = if first_pass.is_empty() {
        pool
    } else {
        first_pass
    };
    let rank = |a: &(&ProfileName, f64, f64), b: &(&ProfileName, f64, f64)| -> Ordering {
        a.2.total_cmp(&b.2)
            .then_with(|| a.1.total_cmp(&b.1))
            .then_with(|| a.0.as_str().cmp(b.0.as_str()))
    };
    let (winner, _, _) = pass.into_iter().min_by(rank)?;
    if live.is_some_and(|l| &l.name == winner) {
        return None;
    }
    Some(winner.clone())
}
