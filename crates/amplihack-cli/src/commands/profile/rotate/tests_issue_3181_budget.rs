//! Issue #3181: which swaps the live login needs, and the identity budget's
//! waiver.

use super::super::decide::Reason;
use super::super::login_identity::Budget;

#[test]
fn only_the_reasons_that_mean_the_live_login_is_near_its_limit_are_needed() {
    for (reason, needed) in [
        (Reason::Threshold, true),
        (Reason::FailedProbe, true),
        (Reason::Projected, true),
        (Reason::Upgrade, false),
        (Reason::OverflowIn, false),
        (Reason::OverflowOut, false),
        (Reason::PlanRoom, false),
    ] {
        assert_eq!(reason.live_needs_it(), needed, "{reason:?}");
    }
}

#[test]
fn a_fresh_budget_is_available_and_a_spent_one_is_not() {
    let b = Budget::new();
    assert!(b.available());
    b.spend();
    assert!(!b.available());
}

#[test]
fn a_waived_budget_is_available_though_spent() {
    let b = Budget::new();
    b.spend();
    b.waive();
    assert!(b.available());
}

#[test]
fn reset_refills_the_budget_and_clears_the_waiver() {
    let b = Budget::new();
    b.spend();
    b.waive();
    b.reset();
    assert!(b.available(), "refilled");
    b.spend();
    assert!(!b.available(), "the waiver did not survive the reset");
}
