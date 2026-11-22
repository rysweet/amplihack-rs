//! Issue #2882: the planner rows a spend limit that states a reset passes
//! through. `plan.rs` logic does not change; the new `Limited::spend_reset`
//! (see `tests_limited_spend_reset.rs`) is OR-ed into `upgrade`'s `seen`
//! argument only, so these tests pin what each promotion does with it.
//!
//! - `upgrade` turns `Skip(ResetTooFar)` into a capacity wait carrying the
//!   stated reset when a login could take over, and leaves it otherwise.
//! - `upgrade_passed_reset` never reads `spend_reset`: a spend limit whose
//!   stated reset has passed is `skipped reason=already_waited` (#2857).
//! - `upgrade_spend` fires only for a spend limit with no reset (#2731).
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_spend_reset;`.

use super::plan::{
    NoWait, WaitFor, upgrade, upgrade_passed_reset, upgrade_spend, wait_for_message,
};
use crate::commands::recipe::run::limit_patterns::{self, Harness};

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;
const SPEND_WEEKLY: &str = "You've hit your individual spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Oct 24, 2pm (America/Los_Angeles)";
const SPEND_NO_RESET: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage";

fn message(text: &str) -> limit_patterns::LimitMessage {
    limit_patterns::limit_message(text.as_bytes(), b"", NOW).expect("a limit message")
}

#[test]
fn a_spend_limit_with_a_weekly_reset_reads_as_spend_with_a_far_reset() {
    let m = message(SPEND_WEEKLY);
    assert!(m.spend);
    let reset = m.reset.expect("a stated reset");
    assert!(reset > NOW + 5 * 3600, "far: {reset}");
    assert_eq!(
        wait_for_message(Some(m), NOW),
        WaitFor::Skip(NoWait::ResetTooFar)
    );
}

#[test]
fn a_far_spend_reset_becomes_a_capacity_wait_when_a_login_could_take_over() {
    let m = message(SPEND_WEEKLY);
    let seen = Some((m.harness, m.reset));
    let w = upgrade(wait_for_message(Some(m), NOW), seen, || true);
    assert_eq!(
        w,
        WaitFor::Capacity {
            harness: Harness::Claude,
            reset: m.reset
        }
    );
}

#[test]
fn a_far_spend_reset_stays_reset_too_far_when_no_login_could_take_over() {
    let m = message(SPEND_WEEKLY);
    let seen = Some((m.harness, m.reset));
    let w = upgrade(wait_for_message(Some(m), NOW), seen, || false);
    assert_eq!(w, WaitFor::Skip(NoWait::ResetTooFar));
}

#[test]
fn the_takeover_question_is_not_asked_for_a_wait_that_is_not_a_skip() {
    let w = WaitFor::Reset {
        harness: Harness::Claude,
        reset: NOW + 60,
    };
    let seen = Some((Harness::Claude, Some(NOW + 60)));
    assert_eq!(upgrade(w, seen, || panic!("not asked")), w);
}

#[test]
fn the_passed_reset_row_never_reads_the_spend_reset() {
    // A spend limit whose stated reset is behind the clock, on a re-entry
    // the ordinary rule refuses, is still not lifted into a capacity wait:
    // `seen` stays `None` for it, whatever `spend_reset` says.
    let w = WaitFor::Reset {
        harness: Harness::Claude,
        reset: NOW - 60,
    };
    assert_eq!(upgrade_passed_reset(w, None, true, NOW), w);
}

#[test]
fn the_login_change_row_still_needs_a_spend_limit_with_no_reset() {
    let none = message(SPEND_NO_RESET);
    assert_eq!(none.reset, None);
    let w = wait_for_message(Some(none), NOW);
    assert_eq!(w, WaitFor::Skip(NoWait::SpendLimit));
    assert_eq!(
        upgrade_spend(w, Some(none.harness), || true),
        WaitFor::LoginChange {
            harness: Harness::Claude
        }
    );
    // A stated reset is not that case: `Limited::spend` is `None` for it.
    assert_eq!(
        upgrade_spend(WaitFor::Skip(NoWait::ResetTooFar), None, || true),
        WaitFor::Skip(NoWait::ResetTooFar)
    );
}

#[test]
fn a_spend_limit_with_no_reset_is_not_promoted_to_capacity() {
    // No reset: `spend_reset` is `None`, so `seen` stays `None` and the
    // skip is left to #2731.
    let m = message(SPEND_NO_RESET);
    assert_eq!(
        upgrade(wait_for_message(Some(m), NOW), None, || true),
        WaitFor::Skip(NoWait::SpendLimit)
    );
}
