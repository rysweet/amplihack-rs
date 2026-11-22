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
//! keeps the API key out while any plan login is unknown. [`room`] is what
//! auto-switch calls and what the `no_room` ledger record means, and issue
//! #2857 left it alone.
//!
//! Moving *to* a login asks more (issue #2857): [`room_under`] adds that its
//! session window is below the rotator's own `--session-at`, so the rotator
//! does not swap onto a login a step would meet a limit on at once. That is
//! the rule [`choose`] filters by, and the one the rotator records on the
//! cycle as `live_room` for the usage-limit wait.

use super::decide::{Measured, Reading};

/// The rotator's record of the logins it measured with no room, kept in its
/// ledger for auto-switch (issue #1804).
#[path = "no_room.rs"]
pub mod no_room;
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
        Reading::Exhausted { .. } | Reading::Failed => false,
    }
}

/// Whether a login using `session`% and `week`% is one the rotator may move
/// *to*: it has [`room`], and its session window is below the rotator's own
/// `--session-at` (issue #2857).
///
/// [`room`] alone says the login can still be used; this says a step moved
/// onto it would not meet its limit at once. On 19 November 2026 the
/// rotator swapped between logins at 98-100% of their session window, each
/// swap woke every waiting run, and the first call after each wake returned
/// a 429. The threshold is strict, as the swap rules' is: a login exactly at
/// `--session-at` is at its threshold, not under it. An unreadable (NaN)
/// session fails both comparisons, so it is never a target.
///
/// The week bound stays at 100%, not `--week-at`: the evidence named the
/// session window, and the two settings answer different questions.
///
/// One definition, shared by the chooser ([`choose`]) and by the rotator's
/// own record of room on the login it made live (`last_cycle.live_room`), so
/// the rotator cannot swap onto a login its ledger then reports as full.
pub fn room_under(session: f64, week: f64, session_at: u8) -> bool {
    room(session, week) && session < f64::from(session_at)
}

/// Whether `r` is a measured reading the rotator may move to at
/// `session_at`. A failed, exhausted or unmeasured reading never is.
pub fn has_room_under(r: &Reading, session_at: u8) -> bool {
    match r {
        Reading::Measured { session, week } => room_under(*session, *week, session_at),
        Reading::Exhausted { .. } | Reading::Failed => false,
    }
}

/// The plan login to move to for room, if any: one pass over the live login
/// and the plan logins, taking those with [`room_under`] (week below 100
/// and session below `--session-at`). Ranked by week, then session, then
/// name. `None` when nothing qualifies, or when the live login ranks first,
/// so the rotator does not swap back and forth between two logins with the
/// same week.
///
/// Issue #2857 removed the second pass, which took any login with room when
/// no login was under the threshold. That pass is what moved steps onto
/// logins at 98-100% of their session window. Where every plan login is at
/// or above `--session-at` there is now no plan-room swap, and the cycle
/// falls to the ALERT or the overflow path that issue #1804 added this to
/// avoid. That is the point: swapping between exhausted logins is not a
/// move, and the ALERT says plainly what the swap used to hide.
///
/// The live login stays in the pool, so a live login that ranks first still
/// returns `None` and nothing flaps.
///
/// `plan` must hold plan logins only: the caller leaves every overflow
/// profile out, so an API-key profile is never chosen.
pub fn choose(s: &Settings, live: Option<&Measured>, plan: &[Measured]) -> Option<ProfileName> {
    let pool = live
        .into_iter()
        .chain(plan)
        .filter_map(|m| match m.reading {
            Reading::Measured { session, week } if room_under(session, week, s.session_at) => {
                Some((&m.name, session, week))
            }
            _ => None,
        });
    let rank = |a: &(&ProfileName, f64, f64), b: &(&ProfileName, f64, f64)| -> Ordering {
        a.2.total_cmp(&b.2)
            .then_with(|| a.1.total_cmp(&b.1))
            .then_with(|| a.0.as_str().cmp(b.0.as_str()))
    };
    let (winner, _, _) = pool.min_by(rank)?;
    if live.is_some_and(|l| &l.name == winner) {
        return None;
    }
    Some(winner.clone())
}
