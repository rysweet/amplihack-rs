//! Issue #2882, as #2911 leaves it: what a spend limit that also states a
//! reset earns.
//!
//! The three promotions this file used to pin (`upgrade`,
//! `upgrade_passed_reset`, `upgrade_spend`) are gone. `wait_for_message`
//! decides each row outright, so the questions those promotions asked -
//! could another login take over *now*, is a rotator running, is this a
//! refused re-entry - no longer stand between a spend limit and its wait.
//! The rows themselves are unchanged in effect, except that the two that
//! used to end the step now wait.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_spend_reset;`.

use super::plan::{WaitFor, Waiting, wait_for_message};
use crate::commands::recipe::run::limit_patterns::{self, Harness, LimitKind};

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;
const SPEND_WEEKLY: &str = "You've hit your individual spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Oct 24, 2pm (America/Los_Angeles)";
const SPEND_SESSION: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your session limit resets 1pm (America/Los_Angeles)";
const SPEND_NO_RESET: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage";

fn message(text: &str) -> limit_patterns::LimitMessage {
    limit_patterns::limit_message(text.as_bytes(), b"", NOW).expect("a limit message")
}

#[test]
fn a_spend_limit_with_a_weekly_reset_reads_as_spend_with_a_far_reset() {
    let m = message(SPEND_WEEKLY);
    assert!(m.spend());
    assert_eq!(m.kind, LimitKind::Spend, "spend wins over the window named");
    let reset = m.reset.expect("a stated reset");
    assert!(reset > NOW + 5 * 3600, "far: {reset}");
}

#[test]
fn issue_2911_a_far_spend_reset_waits_for_capacity_with_nothing_asked_first() {
    // Before #2911 this was a capacity wait only when another login had room
    // at that instant, and `skipped reason=reset_too_far` otherwise. A login
    // can gain room *during* a wait, and the wait's own wake conditions see
    // that, so the question is not asked.
    let m = message(SPEND_WEEKLY);
    assert_eq!(
        wait_for_message(Some(m), NOW),
        WaitFor::Capacity {
            harness: Harness::Claude,
            reset: m.reset,
            why: Waiting::ResetTooFar,
        }
    );
}

#[test]
fn a_spend_limit_stating_a_near_reset_waits_that_window_out() {
    let m = message(SPEND_SESSION);
    let reset = m.reset.expect("a stated session reset");
    assert_eq!(
        wait_for_message(Some(m), NOW),
        WaitFor::Reset {
            harness: Harness::Claude,
            reset,
        },
        "the session window is what stops work first"
    );
}

#[test]
fn issue_2911_a_spend_reset_already_behind_the_clock_waits_for_capacity() {
    // #2857 used to send this row to `skipped reason=already_waited` on a
    // re-entry, which ended the step. A reset behind the clock shows that the
    // stated reset cannot be relied on; it does not show that no wait is
    // owed, so the step waits for a reading instead.
    let m = message(SPEND_SESSION);
    let late = m.reset.expect("a stated session reset") + 60;
    assert_eq!(
        wait_for_message(Some(m), late),
        WaitFor::Capacity {
            harness: Harness::Claude,
            reset: m.reset,
            why: Waiting::PassedReset,
        }
    );
}

#[test]
fn a_spend_limit_with_no_reset_still_waits_for_the_login_to_change() {
    let none = message(SPEND_NO_RESET);
    assert_eq!(none.reset, None);
    assert_eq!(
        wait_for_message(Some(none), NOW),
        WaitFor::LoginChange {
            harness: Harness::Claude
        },
        "no usage window's reset lifts a spend limit, so no reading can"
    );
}

#[test]
fn issue_2911_no_shape_of_spend_limit_ends_the_step() {
    for text in [SPEND_WEEKLY, SPEND_SESSION, SPEND_NO_RESET] {
        let m = message(text);
        for now in [NOW, NOW + 86_400, NOW + 40 * 86_400] {
            let got = wait_for_message(Some(m), now);
            assert!(
                !matches!(got, WaitFor::Skip(_)),
                "{text:?} at {now} gave {got:?}"
            );
        }
    }
}
