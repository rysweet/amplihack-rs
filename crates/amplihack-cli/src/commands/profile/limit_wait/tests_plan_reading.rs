//! Reading-based waits (issues #1618, #2022): when a step was moved off its
//! inherited profile before the first attempt, the inherited account's limit
//! comes from the readings. Since #2022 every future reset is waited for,
//! however far away; `FAR_RESET_THRESHOLD` only decides whether [`upgrade`]
//! may turn it into a capacity wait.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_plan_reading;`.

use super::plan::{FAR_RESET_THRESHOLD, NoWait, WaitFor, upgrade};
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
fn the_far_reset_threshold_is_five_hours() {
    assert_eq!(FAR_RESET_THRESHOLD, 5 * 3600);
}

// The four combinations of near/far and takeover/no takeover, for a reading.
#[test]
fn a_far_reading_becomes_a_capacity_wait_only_when_a_login_could_take_over() {
    let far = NOW + FAR_RESET_THRESHOLD + 1;
    let near = NOW + FAR_RESET_THRESHOLD;
    let seen = |at| Some((Harness::Claude, Some(at)));

    assert_eq!(
        upgrade(reset(far), seen(far), NOW, || true),
        WaitFor::Capacity {
            harness: Harness::Claude,
            reset: Some(far)
        },
        "far, takeover"
    );
    assert_eq!(
        upgrade(reset(far), seen(far), NOW, || false),
        reset(far),
        "far, no takeover: a reset wait on the same login"
    );
    assert_eq!(
        upgrade(reset(near), seen(near), NOW, || true),
        reset(near),
        "near, takeover"
    );
    assert_eq!(
        upgrade(reset(near), seen(near), NOW, || false),
        reset(near),
        "near, no takeover"
    );
}

// S1: a spend limit (nothing seen) is never made a capacity wait.
#[test]
fn a_far_spend_limit_reading_stays_a_reset_wait() {
    let far = NOW + 3 * 86_400;
    assert_eq!(upgrade(reset(far), None, NOW, || true), reset(far));
    assert_eq!(
        upgrade(WaitFor::Skip(NoWait::SpendLimit), None, NOW, || true),
        WaitFor::Skip(NoWait::SpendLimit)
    );
}
