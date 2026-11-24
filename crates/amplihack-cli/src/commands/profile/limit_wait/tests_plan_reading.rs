//! `from_reading` (issues #1618, #2911): when a step was moved off its
//! inherited profile before the first attempt, the inherited account's limit
//! comes from the readings rather than from any output. A reset within five
//! hours is waited for directly; every other reading is a capacity wait, and
//! none of them ends the step.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_plan_reading;`.

use super::plan::{FIVE_HOURS, WaitFor, Waiting, from_reading};
use crate::commands::recipe::run::limit_patterns::Harness;

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;

fn reset(at: i64) -> WaitFor {
    WaitFor::Reset {
        harness: Harness::Claude,
        reset: at,
    }
}

fn capacity(at: i64, why: Waiting) -> WaitFor {
    WaitFor::Capacity {
        harness: Harness::Claude,
        reset: Some(at),
        why,
    }
}

#[test]
fn a_reading_reset_is_waited_for_directly_only_within_five_hours() {
    assert_eq!(
        from_reading(Harness::Claude, NOW + FIVE_HOURS, NOW),
        reset(NOW + FIVE_HOURS)
    );
    assert_eq!(
        from_reading(Harness::Claude, NOW + 1, NOW),
        reset(NOW + 1),
        "one second ahead is still a reset wait"
    );
}

#[test]
fn issue_2911_a_reading_reset_beyond_five_hours_waits_for_capacity() {
    // Before #2911 this was `Skip(ResetTooFar)` and ended the step: a weekly
    // limit on the inherited profile with no other login free stopped the
    // run. It is now a wait that a login gaining room, or the reset itself,
    // can end.
    assert_eq!(
        from_reading(Harness::Claude, NOW + FIVE_HOURS + 1, NOW),
        capacity(NOW + FIVE_HOURS + 1, Waiting::ResetTooFar)
    );
    assert_eq!(
        from_reading(Harness::Claude, NOW + 7 * 86_400, NOW),
        capacity(NOW + 7 * 86_400, Waiting::ResetTooFar),
        "a week away"
    );
}

#[test]
fn issue_2911_a_reading_reset_already_behind_the_clock_waits_for_capacity() {
    // The reading may be minutes stale, so the step waits for a fresh one
    // rather than for a time that has been and gone (#2857). It is never a
    // reset wait, which would have a deadline of `now`.
    for behind in [0, 1, 300, 86_400] {
        assert_eq!(
            from_reading(Harness::Claude, NOW - behind, NOW),
            capacity(NOW - behind, Waiting::PassedReset),
            "{behind}s behind"
        );
    }
}

#[test]
fn a_reading_never_ends_the_step() {
    for reset in [
        i64::MIN,
        NOW - 1,
        NOW,
        NOW + 1,
        NOW + FIVE_HOURS,
        NOW + FIVE_HOURS + 1,
        i64::MAX,
    ] {
        let got = from_reading(Harness::Claude, reset, NOW);
        assert!(
            !matches!(got, WaitFor::Skip(_)),
            "reset {reset} gave {got:?}"
        );
    }
}
