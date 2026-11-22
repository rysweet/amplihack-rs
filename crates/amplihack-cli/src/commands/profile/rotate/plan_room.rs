//! Plan room (issue #1804): whether a plan login can still be used. Pure
//! functions. Issue #2910 moved the choice of a swap target, the plan-room
//! swap included, into the one rule in `decide::target` (see "Rotate target
//! choice" in docs/reference/profile-command.md); what is left here is the
//! meaning of room that auto-switch, the overflow gate and the ledger's
//! `no_room` and `live_room` records read.
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
//! [`room_under`] (issue #2857) adds that the session window is below the
//! rotator's own `--session-at`: the rule the rotator records on the cycle as
//! `live_room` for the usage-limit wait.

use super::decide::Reading;

/// The rotator's record of the logins it measured with no room, kept in its
/// ledger for auto-switch (issue #1804).
#[path = "no_room.rs"]
pub mod no_room;

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
/// The rotator's own record of room on the login it made live
/// (`last_cycle.live_room`) uses it.
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
