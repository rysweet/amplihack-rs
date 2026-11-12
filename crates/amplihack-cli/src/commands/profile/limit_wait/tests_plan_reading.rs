//! `from_reading` (issue #1618): when a step was moved off its inherited
//! profile before the first attempt, the inherited account's limit comes
//! from the readings, and is waited for only within five hours.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_plan_reading;`.

use super::plan::{FIVE_HOURS, NoWait, WaitFor, from_reading};
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
