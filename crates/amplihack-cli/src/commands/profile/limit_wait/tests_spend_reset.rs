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
/// The line that ended run `amp-auto-drive-to-merge-mistt-infra-20261121-213610`
/// on hypatia at 21:45Z on 21 November 2026, quoted in #2911 by the owner's
/// deputy: Claude Code 2.1.33x's per-member spend cap, which names the
/// session window by its newer name. Kept here byte for byte, because both
/// halves of it have to be read correctly for the step to wait: the spend
/// phrase decides the kind, and "5-hour limit resets" is where the reset is.
const SPEND_FIVE_HOUR: &str = "You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit · your 5-hour limit resets 6:30pm (America/Los_Angeles)";

fn message(text: &str) -> limit_patterns::LimitMessage {
    message_at(text, NOW)
}

/// [`message`] read against another instant, for a line whose reset is a
/// time of day: what it means depends on where the clock stands.
fn message_at(text: &str, now: i64) -> limit_patterns::LimitMessage {
    limit_patterns::limit_message(text.as_bytes(), b"", now).expect("a limit message")
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

/// Issue #2911, the deputy's case 1: the message's own words name the
/// session window as "5-hour limit", and the reset after that phrase is the
/// one the step waits for. Read at 15:00, so the stated 6:30pm is 3.5 hours
/// ahead and inside the five-hour ceiling.
#[test]
fn issue_2911_an_individual_spend_cap_states_its_five_hour_reset() {
    // 2026-10-18 15:00:00 -07:00, seven hours after NOW.
    let at = NOW + 7 * 3600;
    let m = message_at(SPEND_FIVE_HOUR, at);
    assert_eq!(
        m.kind,
        LimitKind::Spend,
        "the spend phrase decides the kind, not the window named"
    );
    let reset = m.reset.expect("the reset after \"5-hour limit resets\"");
    assert!(
        reset > at && reset <= at + 5 * 3600,
        "3.5 hours ahead of {at}: {reset}"
    );
    assert_eq!(
        wait_for_message(Some(m), at),
        WaitFor::Reset {
            harness: Harness::Claude,
            reset,
        },
        "a reset the step can sleep to is slept to, spend limit or not"
    );
}

/// The same line read before that reset is more than five hours away: still
/// a wait, for capacity, and never an ending (#2911).
#[test]
fn issue_2911_an_individual_spend_cap_with_a_far_reset_waits_for_capacity() {
    let m = message(SPEND_FIVE_HOUR);
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
fn issue_2911_no_shape_of_spend_limit_ends_the_step() {
    for text in [SPEND_WEEKLY, SPEND_SESSION, SPEND_NO_RESET, SPEND_FIVE_HOUR] {
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

/// Issue #2967: the same spend cap printed with no zone after the reset.
/// No zone is guessed, so the reset is not read, and the wait says why.
const SPEND_NO_ZONE: &str = "You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit · your 5-hour limit resets 11:50pm";

/// Issue #2967: a reset stated with no zone is not read, and the message
/// says so. The wait is for capacity, as the readable form's would be when
/// its reset is unusable, and it names the cause: `reset_without_zone`. A
/// login-change wait, which is what a spend limit with no reset earns, would
/// end only on a login change, though the message names a window that
/// resets.
#[test]
fn a_spend_limit_whose_reset_states_no_zone_waits_for_capacity_and_says_why() {
    let m = message(SPEND_NO_ZONE);
    assert!(m.spend());
    assert_eq!(m.reset, None);
    assert!(m.reset_without_zone);
    assert_eq!(
        wait_for_message(Some(m), NOW),
        WaitFor::Capacity {
            harness: Harness::Claude,
            reset: None,
            why: Waiting::ResetWithoutZone,
        }
    );
    assert_eq!(Waiting::ResetWithoutZone.as_str(), "reset_without_zone");
}

/// A message that states no reset at all, and one whose reset names a zone,
/// are not read as a reset with no zone.
#[test]
fn only_a_bare_clock_time_is_a_reset_without_zone() {
    for text in [SPEND_NO_RESET, SPEND_FIVE_HOUR, SPEND_SESSION, SPEND_WEEKLY] {
        assert!(!message(text).reset_without_zone, "{text}");
    }
    // A zone the table does not know is a stated zone, not a missing one.
    let unknown = SPEND_FIVE_HOUR.replace("America/Los_Angeles", "Mars/Base");
    let m = message(&unknown);
    assert_eq!(m.reset, None);
    assert!(!m.reset_without_zone);
}

/// Issue #2967: a usage limit (not a spend limit) whose reset states no
/// zone names the same cause.
#[test]
fn a_usage_limit_whose_reset_states_no_zone_says_why() {
    let m = message("You've hit your 5-hour limit · resets 8:20pm");
    assert!(!m.spend());
    assert!(m.reset_without_zone);
    assert!(matches!(
        wait_for_message(Some(m), NOW),
        WaitFor::Capacity {
            why: Waiting::ResetWithoutZone,
            reset: None,
            ..
        }
    ));
}
