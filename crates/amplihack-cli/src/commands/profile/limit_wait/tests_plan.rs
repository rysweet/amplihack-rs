//! `wait_for` and `plan` (issue #1618): which limits are waited out (real
//! messages, spend limits, unparsed and far resets, model prose), and the
//! deadline table. The earliest applicable row wins, a tie is named by the
//! earlier row, a reset at or before now means now, and no input can push
//! the deadline past now + 5 h + 120 s.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_plan;`.

use super::plan::{Bound, NoWait, Plan, WaitFor, plan, wait_for};
use crate::commands::recipe::run::limit_patterns::Harness;

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;
/// 2026-10-18 13:00:00 -07:00, exactly five hours on.
const LA_1300: i64 = 1_792_353_600;
const MARGIN: i64 = 120;
const FIVE_HOURS: i64 = 5 * 3600;
const CEILING: i64 = FIVE_HOURS + MARGIN;

const SPEND: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage";
const WEEKLY: &str = "You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)";
const SESSION: &str = "You've hit your session limit · resets 1pm (America/Los_Angeles)";

fn p(reset: i64, max: Option<u64>, step: Option<(i64, u64)>) -> Plan {
    plan(NOW, reset, max, step)
}

// --- wait_for ------------------------------------------------------------

#[test]
fn a_session_limit_within_five_hours_is_waited_for() {
    assert_eq!(
        wait_for(SESSION.as_bytes(), b"", NOW),
        WaitFor::Reset {
            harness: Harness::Claude,
            reset: LA_1300
        }
    );
}

#[test]
fn a_spend_limit_is_never_waited_for() {
    assert_eq!(
        wait_for(SPEND.as_bytes(), b"", NOW),
        WaitFor::Skip(NoWait::SpendLimit)
    );
}

#[test]
fn a_weekly_reset_beyond_five_hours_is_not_waited_for() {
    assert_eq!(
        wait_for(WEEKLY.as_bytes(), b"", NOW),
        WaitFor::Skip(NoWait::ResetTooFar)
    );
    // One second past the cap.
    assert_eq!(
        wait_for(SESSION.as_bytes(), b"", NOW - 1),
        WaitFor::Skip(NoWait::ResetTooFar)
    );
}

#[test]
fn a_message_with_no_readable_reset_is_not_waited_for() {
    for msg in [
        "You've hit your session limit · try again later",
        "You've hit your weekly limit · resets Oct 8 (America/Los_Angeles)",
        "Claude AI usage limit reached|soon",
    ] {
        assert_eq!(
            wait_for(msg.as_bytes(), b"", NOW),
            WaitFor::Skip(NoWait::NoReset),
            "{msg}"
        );
    }
}

#[test]
fn model_output_about_usage_limits_is_not_waited_for() {
    let answer = "I checked the docs: the usage limit resets 8:30am (America/Los_Angeles),\n\
                  and you've hit your session limit · resets 1pm (UTC) is the CLI wording.\n";
    assert_eq!(
        wait_for(b"error: tests failed\n", answer.as_bytes(), NOW),
        WaitFor::Skip(NoWait::NoLimitMessage)
    );
}

// --- plan ----------------------------------------------------------------

#[test]
fn a_future_reset_waits_until_reset_plus_margin() {
    let got = p(NOW + 1800, None, None);
    assert_eq!(
        (got.until, got.bound, got.reset),
        (NOW + 1800 + MARGIN, Bound::ResetMargin, NOW + 1800)
    );
}

#[test]
fn a_reset_at_or_before_now_waits_zero_seconds_and_keeps_the_reset() {
    for reset in [NOW, NOW - 300] {
        let got = p(reset, None, None);
        assert_eq!(
            (got.until, got.bound, got.reset),
            (NOW, Bound::ResetMargin, reset)
        );
    }
}

#[test]
fn a_maximum_only_shortens_the_wait() {
    let got = p(NOW + 1800, Some(600), None);
    assert_eq!((got.until, got.bound), (NOW + 600, Bound::MaxWait));
    let got = p(NOW + 1800, Some(7200), None);
    assert_eq!((got.until, got.bound), (NOW + 1920, Bound::ResetMargin));
    let got = p(NOW + 1800, Some(0), None);
    assert_eq!((got.until, got.bound), (NOW, Bound::MaxWait));
}

#[test]
fn a_tie_is_named_by_the_earlier_row() {
    let got = p(NOW + 480, Some(600), None);
    assert_eq!((got.until, got.bound), (NOW + 600, Bound::ResetMargin));
    let got = p(NOW - 1, Some(0), None);
    assert_eq!((got.until, got.bound), (NOW, Bound::ResetMargin));
    let got = p(NOW + 1800, Some(600), Some((NOW, 660)));
    assert_eq!((got.until, got.bound), (NOW + 600, Bound::MaxWait));
}

#[test]
fn a_step_timeout_ends_the_wait_sixty_seconds_before_it() {
    let got = p(NOW + 1800, None, Some((NOW - 100, 1000)));
    assert_eq!((got.until, got.bound), (NOW + 840, Bound::StepTimeout));
    for t in [1u64, 30, 60] {
        let got = p(NOW + 1800, None, Some((NOW, t)));
        assert_eq!((got.until, got.bound), (NOW, Bound::StepTimeout), "{t}");
    }
    let got = p(NOW + 1800, None, Some((NOW - 5000, 1000)));
    assert_eq!((got.until, got.bound), (NOW, Bound::StepTimeout), "used up");
    let got = p(NOW + 1800, None, Some((NOW, 0)));
    assert_eq!((got.until, got.bound), (NOW + 1920, Bound::ResetMargin));
}

#[test]
fn extreme_inputs_saturate_without_panicking() {
    let got = p(i64::MAX, Some(u64::MAX), Some((i64::MAX, u64::MAX)));
    assert_eq!(got.until, NOW + CEILING);
    let got = p(i64::MIN, None, Some((i64::MIN, u64::MAX)));
    assert_eq!(got.until, NOW);
    let got = plan(i64::MAX - 10, i64::MAX, Some(u64::MAX), None);
    assert!(got.until >= i64::MAX - 10);
    let got = plan(i64::MIN, i64::MIN, None, Some((i64::MIN, 1)));
    assert_eq!(got.until, i64::MIN, "never earlier than now");
}

#[test]
fn until_is_always_between_now_and_the_ceiling() {
    let resets = [
        i64::MIN,
        NOW - 1,
        NOW,
        NOW + 1,
        NOW + FIVE_HOURS,
        NOW + FIVE_HOURS + 1,
        i64::MAX,
    ];
    let maxes = [None, Some(0), Some(1800), Some(18_120), Some(u64::MAX)];
    let steps = [
        None,
        Some((NOW, 0)),
        Some((NOW, 61)),
        Some((i64::MIN, u64::MAX)),
        Some((i64::MAX, u64::MAX)),
    ];
    for r in resets {
        for m in maxes {
            for s in steps {
                let got = p(r, m, s);
                assert!(
                    got.until >= NOW && got.until - NOW <= CEILING,
                    "reset {r} max {m:?} step {s:?}"
                );
            }
        }
    }
}
