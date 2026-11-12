//! The plan-capacity gate of the overflow policy, split from
//! [`super`]: when a plan login has capacity, when it regains it,
//! and the shared [`Gate`] built from the rotator's settings.
use super::super::decide::{Reading, Windows};
use super::super::settings::Settings;
use crate::commands::profile::overflow::Gate;

/// A plan login with capacity: an `ok` probe that passes
/// [`Gate::plan_capacity`], the test auto-switch also applies.
pub fn plan_usable(s: &Settings, r: &Reading) -> bool {
    match r {
        Reading::Measured { session, week } => gate(s).plan_capacity(*session, *week),
        Reading::Exhausted { .. } | Reading::Failed => false,
    }
}

/// When a plan login without capacity has it again: the reset of the window
/// at its threshold, or the later of the two when both are. `None` when
/// unknown, including every failed probe.
pub fn regains(s: &Settings, r: &Reading, w: &Windows) -> Option<i64> {
    let (session_used, week_used) = match r {
        Reading::Failed => return None,
        Reading::Measured { session, week } => (Some(*session), Some(*week)),
        Reading::Exhausted { .. } => (w.session.map(|x| x.used), w.week.map(|x| x.used)),
    };
    let g = gate(s);
    let session_hit = session_used.is_some_and(|u| g.at_threshold(u, 0.0).0);
    let week_hit = week_used.is_some_and(|u| g.at_threshold(0.0, u).1);
    let session_reset = w.session.and_then(|x| x.resets_at);
    let week_reset = w.week.and_then(|x| x.resets_at);
    match (session_hit, week_hit) {
        (true, false) => session_reset,
        (false, true) => week_reset,
        (true, true) => Some(session_reset?.max(week_reset?)),
        (false, false) => None,
    }
}

/// The earliest known time any of `pool` regains capacity. Unknown resets
/// are left out, so they never shorten the wait; `None` when no reset in
/// `pool` is known, which [`Gate::far`] treats as far only by opt-in.
pub fn earliest_plan_reset<'a>(
    s: &Settings,
    pool: impl IntoIterator<Item = (&'a Reading, &'a Windows)>,
) -> Option<i64> {
    pool.into_iter().filter_map(|(r, w)| regains(s, r, w)).min()
}

/// The rotator's [`Gate`]: `--overflow-wait`, `--overflow-unknown-reset`,
/// `--session-at` and `--week-at`. The running rotator records it for
/// auto-switch (`gate_record`).
pub fn gate(s: &Settings) -> Gate {
    Gate {
        wait: s.overflow_wait,
        unknown_reset: s.overflow_unknown_reset,
        session_at: s.session_at,
        week_at: s.week_at,
    }
}
