//! `later` and `from_reading` (issue #1618): when a step's limited attempts
//! ran under several profiles, the wait is for the latest reset among them,
//! in either order, and a limit no wait lifts stops the wait whichever
//! attempt saw it.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_plan_later;`.

use super::plan::{FIVE_HOURS, NoWait, WaitFor, from_reading, later};
use crate::commands::recipe::run::limit_patterns::Harness;

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;

fn reset(at: i64) -> WaitFor {
    WaitFor::Reset {
        harness: Harness::Claude,
        reset: at,
    }
}

#[test]
fn the_later_reset_wins_in_either_order() {
    let (early, late) = (reset(NOW + 1800), reset(NOW + 5400));
    assert_eq!(later(early, late), late);
    assert_eq!(later(late, early), late);
}

#[test]
fn a_limit_no_wait_lifts_stops_the_wait_whichever_attempt_saw_it() {
    let ok = reset(NOW + 1800);
    let spend = WaitFor::Skip(NoWait::SpendLimit);
    let far = WaitFor::Skip(NoWait::ResetTooFar);
    assert_eq!(later(ok, spend), spend);
    assert_eq!(later(spend, ok), spend);
    assert_eq!(later(spend, far), spend, "the earlier attempt names why");
}

#[test]
fn a_reading_reset_is_waited_for_only_within_five_hours() {
    assert_eq!(
        from_reading(Harness::Claude, NOW + FIVE_HOURS, NOW),
        reset(NOW + FIVE_HOURS)
    );
    assert_eq!(
        from_reading(Harness::Claude, NOW + FIVE_HOURS + 1, NOW),
        WaitFor::Skip(NoWait::ResetTooFar)
    );
}
