//! Issue #2731: the login-change wait's plan, and which readings may end it.
//!
//! An organisation spend limit that states no reset is not lifted by waiting,
//! and another login of the same organisation would not lift it either, so a
//! capacity *reading* does not apply. The login that the rotator moves in
//! next may belong to another organisation, so that is what the step waits
//! for.
//!
//! Issue #2911: the wait does not ask whether a rotator is running before
//! it starts, and it has no deadline. The login changing is the event it
//! waits for, and with no rotator running a person makes that event
//! (`amplihack profile switch`, or a login in the step's config directory);
//! no timer runs the step again on the spent login in the meantime. It is
//! reached straight from [`wait_for_message`], with no promotion in
//! between. The plan keeps [`PlanKind::Reset`], so the wait never consults
//! the stored readings.
//!
//! Wired from `limit_wait/mod.rs` with
//! `#[cfg(test)] mod tests_plan_login_change;`.

use super::plan::{Bound, PlanKind, WaitFor, plan_login_change, wait_for_message};
use crate::commands::recipe::run::limit_patterns::{self, Harness};

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;

const SPEND: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage";

fn p() -> super::plan::Plan {
    plan_login_change()
}

// --- the deadline ---------------------------------------------------------

/// Issues #2911 and #3037: the limit stated no reset, so nothing timed ends
/// this wait. Neither a configured length nor the step timeout has a row.
#[test]
fn issue_2911_a_login_change_wait_has_no_deadline() {
    let plan = p();
    assert_eq!((plan.until, plan.bound), (None, Bound::Unbounded));
}

// --- the plan's shape -----------------------------------------------------

#[test]
fn there_is_no_reset_to_show() {
    assert_eq!(
        p().reset,
        None,
        "the limit stated none, so the start line says reset=none"
    );
}

#[test]
fn the_kind_is_the_reset_kind() {
    // The kind says what ending the wait means: a retry under the login now
    // live (issue #2911). Which readings the wait consults is
    // `Plan::capacity_may_end`'s answer, asserted below.
    assert_eq!(p().kind, PlanKind::Reset);
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
    let plan = super::plan::plan_capacity(NOW, None);
    assert!(plan.capacity_may_end(), "#1801: that is what it waits for");
}

#[test]
fn a_login_change_plan_may_not_be_ended_by_any_capacity_reading() {
    assert!(
        !p().capacity_may_end(),
        "no reading says whether the next login's organisation has credit"
    );
}

#[test]
fn the_flag_rides_with_the_plan_and_not_with_its_kind() {
    // Both plans are `PlanKind::Reset`, because that is what ending them
    // means; only the login-change one suppresses the capacity sources.
    let reset = super::plan::plan(NOW, NOW + 1800);
    let login_change = p();
    assert_eq!(reset.kind, login_change.kind, "the same kind");
    assert_ne!(
        reset.capacity_may_end(),
        login_change.capacity_may_end(),
        "and still different answers"
    );
}
