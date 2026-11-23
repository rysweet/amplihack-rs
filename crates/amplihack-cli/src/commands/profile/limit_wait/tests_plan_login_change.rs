//! Issue #2731: the login-change wait's plan, and which readings may end it.
//!
//! An organisation spend limit that states no reset is not lifted by waiting,
//! and another login of the same organisation would not lift it either, so a
//! capacity *reading* does not apply. The login that the rotator moves in
//! next may belong to another organisation, so that is what the step waits
//! for.
//!
//! Issue #2911 changed two things about it. The wait no longer asks whether
//! a rotator is running before it starts: with none running nothing ends it
//! early, so it runs its length and the step retries, where before it ended
//! the step with `failed reason=spend_limit`. And it is reached straight from
//! [`wait_for_message`], with no promotion in between, which is why
//! `upgrade_spend` is gone. The plan still keeps [`PlanKind::Reset`], so the
//! wait itself is unchanged and never consults the stored readings.
//!
//! Wired from `limit_wait/mod.rs` with
//! `#[cfg(test)] mod tests_plan_login_change;`.

use super::plan::{
    Bound, CAPACITY_DEFAULT_MAX, MIN_WAIT_SECS, PlanKind, WaitFor, plan_login_change,
    wait_for_message,
};
use crate::commands::recipe::run::limit_patterns::{self, Harness};

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;

const SPEND: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage";

fn p(max: Option<u64>) -> super::plan::Plan {
    plan_login_change(NOW, max)
}

// --- the deadline ---------------------------------------------------------

#[test]
fn the_default_length_is_the_capacity_waits_six_hours() {
    let plan = p(None);
    assert_eq!(plan.until, NOW + CAPACITY_DEFAULT_MAX as i64);
    assert_eq!(plan.bound, Bound::MaxWait);
}

#[test]
fn the_maximum_sets_the_length() {
    let plan = p(Some(600));
    assert_eq!(plan.until, NOW + 600);
    assert_eq!(plan.bound, Bound::MaxWait);
}

#[test]
fn issue_2911_the_shortest_length_is_one_floor_and_never_zero() {
    // A zero can no longer reach here: `settings` floors a configured value
    // at `MIN_WAIT_SECS` where it reads it, so the shortest wait this plan
    // can be asked for still ends strictly after `now`.
    let plan = p(Some(MIN_WAIT_SECS));
    assert_eq!(plan.until, NOW + i64::try_from(MIN_WAIT_SECS).unwrap());
    assert!(
        plan.until > NOW,
        "no wait of no length, and so no busy loop"
    );
}

/// Issue #3037: nothing but the wait's own length sets
/// this deadline. The step timeout once had a row here, which ended the
/// wait early and failed the step while its spend limit still stood.
#[test]
fn issue_3037_only_the_wait_length_sets_the_deadline() {
    for max in [Some(MIN_WAIT_SECS), Some(240), Some(600), None] {
        let plan = p(max);
        let len = i64::try_from(max.unwrap_or(CAPACITY_DEFAULT_MAX)).unwrap();
        assert_eq!(
            (plan.until, plan.bound),
            (NOW + len, Bound::MaxWait),
            "{max:?}"
        );
    }
}

#[test]
fn a_very_large_maximum_saturates_rather_than_overflowing() {
    let plan = p(Some(u64::MAX));
    assert!(plan.until > NOW);
    assert_eq!(plan.bound, Bound::MaxWait);
}

// --- the plan's shape -----------------------------------------------------

#[test]
fn there_is_no_reset_to_show() {
    assert_eq!(
        p(Some(600)).reset,
        None,
        "the limit stated none, so the start line says reset=none"
    );
}

#[test]
fn the_deadline_means_what_a_reset_waits_deadline_means() {
    // The kind says what reaching the deadline means, and nothing else:
    // `wait::End::at_deadline` maps `Reset` to `MaxReached`, which
    // `after_limit` turns into a retry (issue #2911). Which readings the
    // wait consults is `Plan::capacity_may_end`'s answer, asserted below.
    assert_eq!(p(Some(600)).kind, PlanKind::Reset);
}

#[test]
fn there_is_no_five_hour_ceiling() {
    let eight_hours = 8 * 3600;
    assert_eq!(p(Some(eight_hours as u64)).until, NOW + eight_hours);
}

// --- how a spend limit reaches this wait ----------------------------------

#[test]
fn a_spend_limit_stating_no_reset_is_a_login_change_wait() {
    let message = limit_patterns::limit_message(SPEND.as_bytes(), b"", NOW);
    assert_eq!(
        wait_for_message(message, NOW),
        WaitFor::LoginChange {
            harness: Harness::Claude
        }
    );
}

/// Issue #2911: nothing is asked about a rotator, about another login having
/// room, or about anything else before the wait starts. The planner takes
/// the message and the clock, and that is the whole of its input: there is
/// no argument it could consult, which is what removes the
/// `skipped reason=spend_limit` ending the issue reports.
#[test]
fn issue_2911_nothing_gates_the_start_of_a_login_change_wait() {
    let message = limit_patterns::limit_message(SPEND.as_bytes(), b"", NOW);
    for now in [NOW, NOW + 10 * 86_400, 0] {
        let got = wait_for_message(message, now);
        assert!(
            matches!(got, WaitFor::LoginChange { .. }),
            "at {now} it gave {got:?}"
        );
    }
}

// --- which readings may end the wait (issue #2731, deputy point 4) ---------
//
// A login-change wait is waiting for an organisation's credit to be replaced
// by another organisation's. No capacity reading says anything about that: a
// session or weekly window that looks healthy is exactly what an exhausted
// credit leaves behind. `Plan::capacity_may_end` is what the wait loop asks
// before it consults either capacity source, and it is false for this plan
// and true for every other, so #2717's rule stands where it applies and is
// suppressed only here.

#[test]
fn a_reset_plan_may_be_ended_by_a_capacity_reading() {
    let plan = super::plan::plan(NOW, NOW + 1800);
    assert!(
        plan.capacity_may_end(),
        "#2717: another login with room ends a reset wait"
    );
}

#[test]
fn a_capacity_plan_may_be_ended_by_a_capacity_reading() {
    let plan = super::plan::plan_capacity(NOW, None, None);
    assert!(plan.capacity_may_end(), "#1801: that is what it waits for");
}

#[test]
fn a_login_change_plan_may_not_be_ended_by_any_capacity_reading() {
    assert!(
        !p(Some(600)).capacity_may_end(),
        "no reading says whether the next login's organisation has credit"
    );
}

#[test]
fn the_flag_rides_with_the_plan_and_not_with_its_kind() {
    // Both plans are `PlanKind::Reset`, because that is what their deadline
    // means; only the login-change one suppresses the capacity sources.
    let reset = super::plan::plan(NOW, NOW + 1800);
    let login_change = p(Some(600));
    assert_eq!(reset.kind, login_change.kind, "the same kind");
    assert_ne!(
        reset.capacity_may_end(),
        login_change.capacity_may_end(),
        "and still different answers"
    );
}
