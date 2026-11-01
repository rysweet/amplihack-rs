//! `wait_for` and `plan` (issue #1618): which limits are waited out (real
//! messages, spend limits, unparsed and far resets, model prose), and the
//! deadline table. The earliest applicable row wins, a tie is named by the
//! earlier row, a reset at or before now means now, and no input can push
//! the deadline past now + 5 h + 120 s.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_plan;`.

use super::plan::{
    Bound, CAPACITY_DEFAULT_MAX, NoWait, Plan, PlanKind, WaitFor, plan, plan_capacity, upgrade,
    wait_for_message,
};
use crate::commands::recipe::run::limit_patterns::{self, Harness};

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;
/// 2026-10-18 13:00:00 -07:00, exactly five hours on.
const LA_1300: i64 = 1_792_353_600;
const MARGIN: i64 = 120;
const FIVE_HOURS: i64 = 5 * 3600;
const CEILING: i64 = FIVE_HOURS + MARGIN;

const SPEND: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage";
const SPEND_SESSION: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your session limit resets 1pm (America/Los_Angeles)";
const SPEND_WEEKLY: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Oct 8, 12am (America/Los_Angeles)";
const WEEKLY: &str = "You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)";
const SESSION: &str = "You've hit your session limit · resets 1pm (America/Los_Angeles)";

fn wait_for(stderr: &[u8], stdout: &[u8], now: i64) -> WaitFor {
    wait_for_message(limit_patterns::limit_message(stderr, stdout, now), now)
}

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
fn a_spend_limit_is_waited_for_only_when_it_states_a_near_reset() {
    assert_eq!(
        wait_for(SPEND.as_bytes(), b"", NOW),
        WaitFor::Skip(NoWait::SpendLimit),
        "no reset stated"
    );
    assert_eq!(
        wait_for(SPEND_SESSION.as_bytes(), b"", NOW),
        WaitFor::Reset {
            harness: Harness::Claude,
            reset: LA_1300
        },
        "the session limit resets within five hours"
    );
    assert_eq!(
        wait_for(SPEND_WEEKLY.as_bytes(), b"", NOW),
        WaitFor::Skip(NoWait::ResetTooFar)
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
        (NOW + 1800 + MARGIN, Bound::ResetMargin, Some(NOW + 1800))
    );
}

#[test]
fn a_reset_at_or_before_now_waits_zero_seconds_and_keeps_the_reset() {
    for reset in [NOW, NOW - 300] {
        let got = p(reset, None, None);
        assert_eq!(
            (got.until, got.bound, got.reset),
            (NOW, Bound::ResetMargin, Some(reset))
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

// --- upgrade (issue #1801) -----------------------------------------------

const SEEN_FAR: Option<(Harness, Option<i64>)> = Some((Harness::Claude, Some(NOW + 86_400)));
const SEEN_NONE: Option<(Harness, Option<i64>)> = Some((Harness::Claude, None));

#[test]
fn a_far_or_unreadable_reset_becomes_a_capacity_wait_when_a_login_could_take_over() {
    assert_eq!(
        upgrade(WaitFor::Skip(NoWait::ResetTooFar), SEEN_FAR, || true),
        WaitFor::Capacity {
            harness: Harness::Claude,
            reset: Some(NOW + 86_400)
        }
    );
    assert_eq!(
        upgrade(WaitFor::Skip(NoWait::NoReset), SEEN_NONE, || true),
        WaitFor::Capacity {
            harness: Harness::Claude,
            reset: None
        }
    );
}

#[test]
fn no_login_to_take_over_keeps_the_skip() {
    for why in [NoWait::ResetTooFar, NoWait::NoReset] {
        assert_eq!(
            upgrade(WaitFor::Skip(why), SEEN_FAR, || false),
            WaitFor::Skip(why)
        );
    }
}

#[test]
fn nothing_seen_keeps_the_skip_even_with_a_login_to_take_over() {
    // A spend limit reports nothing as seen, so a stated weekly reset stays
    // `reset_too_far` (D3).
    assert_eq!(
        upgrade(WaitFor::Skip(NoWait::ResetTooFar), None, || true),
        WaitFor::Skip(NoWait::ResetTooFar)
    );
}

#[test]
fn only_a_far_or_unreadable_reset_is_upgraded() {
    for why in [
        NoWait::NoLimitMessage,
        NoWait::SpendLimit,
        NoWait::PipedPrompt,
    ] {
        assert_eq!(
            upgrade(WaitFor::Skip(why), SEEN_FAR, || true),
            WaitFor::Skip(why),
            "{why:?}"
        );
    }
}

#[test]
fn a_reset_wait_is_never_upgraded() {
    let near = WaitFor::Reset {
        harness: Harness::Claude,
        reset: NOW + 1800,
    };
    assert_eq!(upgrade(near, SEEN_FAR, || true), near);
}

#[test]
fn the_takeover_check_runs_only_for_a_limit_that_qualifies() {
    let calls = std::cell::Cell::new(0);
    let check = || {
        calls.set(calls.get() + 1);
        true
    };
    let near = WaitFor::Reset {
        harness: Harness::Claude,
        reset: NOW + 1800,
    };
    upgrade(near, SEEN_FAR, check);
    upgrade(WaitFor::Skip(NoWait::ResetTooFar), None, check);
    upgrade(WaitFor::Skip(NoWait::PipedPrompt), SEEN_FAR, check);
    assert_eq!(
        calls.get(),
        0,
        "no store read for a limit that cannot qualify"
    );
    upgrade(WaitFor::Skip(NoWait::ResetTooFar), SEEN_FAR, check);
    assert_eq!(calls.get(), 1);
}

// --- plan_capacity (issue #1801) -----------------------------------------

#[test]
fn a_reset_plan_is_of_the_reset_kind() {
    assert_eq!(p(NOW + 1800, None, None).kind, PlanKind::Reset);
}

#[test]
fn a_capacity_wait_lasts_the_maximum_with_no_five_hour_ceiling() {
    let got = plan_capacity(NOW, Some(NOW + 86_400), Some(21_600), None);
    assert_eq!(
        (got.until, got.bound, got.kind, got.reset),
        (
            NOW + 21_600,
            Bound::MaxWait,
            PlanKind::Capacity,
            Some(NOW + 86_400)
        )
    );
    let got = plan_capacity(NOW, None, Some(86_400), None);
    assert_eq!((got.until, got.reset), (NOW + 86_400, None));
}

#[test]
fn a_step_timeout_ends_a_capacity_wait_sixty_seconds_before_it() {
    let got = plan_capacity(NOW, None, Some(21_600), Some((NOW - 100, 1000)));
    assert_eq!((got.until, got.bound), (NOW + 840, Bound::StepTimeout));
    let got = plan_capacity(NOW, None, Some(21_600), Some((NOW, 0)));
    assert_eq!(
        (got.until, got.bound),
        (NOW + 21_600, Bound::MaxWait),
        "a timeout of 0 does not apply"
    );
    let got = plan_capacity(NOW, None, Some(21_600), Some((NOW - 5000, 1000)));
    assert_eq!(
        (got.until, got.bound),
        (NOW, Bound::StepTimeout),
        "used up: never earlier than now"
    );
}

#[test]
fn a_tie_on_a_capacity_wait_is_named_max_wait() {
    let got = plan_capacity(NOW, None, Some(600), Some((NOW, 660)));
    assert_eq!((got.until, got.bound), (NOW + 600, Bound::MaxWait));
}

#[test]
fn a_zero_maximum_is_an_immediate_capacity_deadline() {
    let got = plan_capacity(NOW, None, Some(0), None);
    assert_eq!((got.until, got.bound), (NOW, Bound::MaxWait));
}

#[test]
fn extreme_capacity_inputs_saturate_without_panicking() {
    let got = plan_capacity(NOW, None, Some(u64::MAX), None);
    assert_eq!(got.until, i64::MAX, "no overflow, no ceiling");
    let got = plan_capacity(NOW, None, Some(u64::MAX), Some((NOW, 1000)));
    assert_eq!((got.until, got.bound), (NOW + 940, Bound::StepTimeout));
    let got = plan_capacity(
        i64::MAX - 10,
        Some(i64::MAX),
        Some(u64::MAX),
        Some((i64::MAX, u64::MAX)),
    );
    assert!(got.until >= i64::MAX - 10);
    let got = plan_capacity(i64::MIN, None, Some(0), Some((i64::MIN, 1)));
    assert_eq!(got.until, i64::MIN, "never earlier than now");
}

#[test]
fn the_capacity_default_is_the_schema_default() {
    let key = crate::user_config::schema::KEYS
        .iter()
        .find(|k| k.name == super::settings::MAX_WAIT_KEY)
        .expect("the wait cap is a config key");
    assert_eq!(key.default, Some(CAPACITY_DEFAULT_MAX.to_string().as_str()));
}

// AC5: with the setting unset, a capacity wait lasts the six-hour default,
// and a step timeout that ends sooner ends it sooner.
#[test]
fn ac5_an_unset_maximum_gives_a_capacity_wait_of_six_hours() {
    let got = plan_capacity(NOW, None, None, None);
    assert_eq!(
        (got.until, got.bound, got.kind),
        (NOW + 21_600, Bound::MaxWait, PlanKind::Capacity)
    );
    let got = plan_capacity(NOW, Some(NOW + 3 * 86_400), None, Some((NOW, 3600)));
    assert_eq!((got.until, got.bound), (NOW + 3540, Bound::StepTimeout));
    let got = plan_capacity(NOW, None, None, Some((NOW, 30_000)));
    assert_eq!(
        (got.until, got.bound),
        (NOW + 21_600, Bound::MaxWait),
        "a later step timeout does not lengthen the wait"
    );
}

// The six-hour capacity default never reaches a reset wait: with no maximum
// a reset wait still ends at its reset plus margin, inside five hours.
#[test]
fn the_capacity_default_does_not_bound_a_reset_wait() {
    let got = p(NOW + 4 * 3600, None, None);
    assert_eq!(
        (got.until, got.bound, got.kind),
        (NOW + 4 * 3600 + MARGIN, Bound::ResetMargin, PlanKind::Reset)
    );
}

// V1: one second of maximum is one second of wait, and the largest value
// the schema accepts saturates rather than wrapping below now.
#[test]
fn capacity_maximum_boundaries() {
    assert_eq!(plan_capacity(NOW, None, Some(1), None).until, NOW + 1);
    let big = u64::try_from(i64::MAX).unwrap();
    for max in [big - 1, big, big + 1] {
        let got = plan_capacity(NOW, None, Some(max), None);
        assert!(got.until >= NOW, "max {max}: {got:?}");
        assert_eq!(got.bound, Bound::MaxWait);
    }
}
