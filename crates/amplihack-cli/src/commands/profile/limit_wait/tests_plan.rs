//! `wait_for` and `plan` (issues #1618, #2022): which limits are waited out
//! (real messages, spend limits, unparsed and far resets, model prose), and
//! the deadline table. The earliest applicable row wins, a tie is named by the
//! earlier row and a reset at or before now means now. Since #2022 there is
//! no five-hour ceiling: a far reset is waited for until the reset plus the
//! margin unless a maximum or step timeout ends the wait sooner.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_plan;`.

use super::plan::{
    Bound, NoWait, Plan, PlanKind, WaitFor, plan, plan_capacity, upgrade, wait_for_message,
};
use crate::commands::recipe::run::limit_patterns::{self, Harness};

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;
/// 2026-10-18 13:00:00 -07:00, exactly five hours on.
const LA_1300: i64 = 1_792_353_600;
const MARGIN: i64 = 120;
const FIVE_HOURS: i64 = 5 * 3600;

const SPEND: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage";
const SPEND_SESSION: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your session limit resets 1pm (America/Los_Angeles)";
const SPEND_WEEKLY: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Oct 8, 12am (America/Los_Angeles)";
const WEEKLY: &str = "You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)";
const SESSION: &str = "You've hit your session limit · resets 1pm (America/Los_Angeles)";
/// The weekly reset in `WEEKLY` / `SPEND_WEEKLY` as read at `NOW`.
fn weekly_reset() -> i64 {
    match wait_for(WEEKLY.as_bytes(), b"", NOW) {
        WaitFor::Reset { reset, .. } => reset,
        other => panic!("a weekly limit must be a reset wait, got {other:?}"),
    }
}

fn wait_for(stderr: &[u8], stdout: &[u8], now: i64) -> WaitFor {
    wait_for_message(limit_patterns::limit_message(stderr, stdout, now))
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
        WaitFor::Reset {
            harness: Harness::Claude,
            reset: weekly_reset()
        },
        "issue #2022: a stated weekly reset on a spend line is waited for on the same login"
    );
}

// Issue #2022: a reset more than five hours away is a wait, not a skip.
#[test]
fn a_weekly_reset_beyond_five_hours_is_waited_for() {
    let reset = weekly_reset();
    assert!(reset > NOW + FIVE_HOURS, "the fixture is a far reset");
    // One second past the former cap.
    assert_eq!(
        wait_for(SESSION.as_bytes(), b"", NOW - 1),
        WaitFor::Reset {
            harness: Harness::Claude,
            reset: LA_1300
        }
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
        (
            Some(NOW + 1800 + MARGIN),
            Bound::ResetMargin,
            Some(NOW + 1800)
        )
    );
}

#[test]
fn a_reset_at_or_before_now_waits_zero_seconds_and_keeps_the_reset() {
    for reset in [NOW, NOW - 300] {
        let got = p(reset, None, None);
        assert_eq!(
            (got.until, got.bound, got.reset),
            (Some(NOW), Bound::ResetMargin, Some(reset))
        );
    }
}

#[test]
fn a_maximum_only_shortens_the_wait() {
    let got = p(NOW + 1800, Some(600), None);
    assert_eq!((got.until, got.bound), (Some(NOW + 600), Bound::MaxWait));
    let got = p(NOW + 1800, Some(7200), None);
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 1920), Bound::ResetMargin)
    );
    let got = p(NOW + 1800, Some(0), None);
    assert_eq!((got.until, got.bound), (Some(NOW), Bound::MaxWait));
}

#[test]
fn a_tie_is_named_by_the_earlier_row() {
    let got = p(NOW + 480, Some(600), None);
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 600), Bound::ResetMargin)
    );
    let got = p(NOW - 1, Some(0), None);
    assert_eq!((got.until, got.bound), (Some(NOW), Bound::ResetMargin));
    let got = p(NOW + 1800, Some(600), Some((NOW, 660)));
    assert_eq!((got.until, got.bound), (Some(NOW + 600), Bound::MaxWait));
}

#[test]
fn a_step_timeout_ends_the_wait_sixty_seconds_before_it() {
    let got = p(NOW + 1800, None, Some((NOW - 100, 1000)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 840), Bound::StepTimeout)
    );
    for t in [1u64, 30, 60] {
        let got = p(NOW + 1800, None, Some((NOW, t)));
        assert_eq!(
            (got.until, got.bound),
            (Some(NOW), Bound::StepTimeout),
            "{t}"
        );
    }
    let got = p(NOW + 1800, None, Some((NOW - 5000, 1000)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW), Bound::StepTimeout),
        "used up"
    );
    let got = p(NOW + 1800, None, Some((NOW, 0)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 1920), Bound::ResetMargin)
    );
}

#[test]
fn extreme_inputs_saturate_without_panicking() {
    let got = p(i64::MAX, Some(u64::MAX), Some((i64::MAX, u64::MAX)));
    assert_eq!(
        (got.until, got.bound),
        (Some(i64::MAX), Bound::ResetMargin),
        "no ceiling: reset + margin saturates at i64::MAX"
    );
    let got = p(i64::MIN, None, Some((i64::MIN, u64::MAX)));
    assert_eq!(got.until, Some(NOW));
    let got = plan(i64::MAX - 10, i64::MAX, Some(u64::MAX), None);
    assert!(got.until.is_some_and(|u| u >= i64::MAX - 10));
    let got = plan(i64::MIN, i64::MIN, None, Some((i64::MIN, 1)));
    assert_eq!(got.until, Some(i64::MIN), "never earlier than now");
}

// Issue #2022: a reset wait is always bounded (by its own reset), never
// before now, and never later than its reset plus the margin.
#[test]
fn until_is_always_between_now_and_reset_plus_margin() {
    let resets = [
        i64::MIN,
        NOW - 1,
        NOW,
        NOW + 1,
        NOW + FIVE_HOURS,
        NOW + FIVE_HOURS + 1,
        NOW + 8 * 3600,
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
                let until = got.until.expect("a reset wait always has a deadline");
                assert!(
                    until >= NOW && until <= r.max(NOW).saturating_add(MARGIN),
                    "reset {r} max {m:?} step {s:?}: {got:?}"
                );
            }
        }
    }
}

// --- AC6: far resets (issue #2022) ----------------------------------------

const EIGHT_HOURS: i64 = 8 * 3600;

#[test]
fn ac6_a_reset_eight_hours_ahead_with_no_maximum_waits_for_the_reset() {
    let got = p(NOW + EIGHT_HOURS, None, None);
    assert_eq!(
        (got.until, got.bound, got.kind, got.reset),
        (
            Some(NOW + EIGHT_HOURS + MARGIN),
            Bound::ResetMargin,
            PlanKind::Reset,
            Some(NOW + EIGHT_HOURS)
        )
    );
}

#[test]
fn ac6_an_explicit_maximum_bounds_a_far_reset_wait() {
    let got = p(NOW + EIGHT_HOURS, Some(600), None);
    assert_eq!((got.until, got.bound), (Some(NOW + 600), Bound::MaxWait));
}

#[test]
fn ac6_a_step_timeout_bounds_a_far_reset_wait() {
    let got = p(NOW + EIGHT_HOURS, None, Some((NOW, 3600)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 3540), Bound::StepTimeout)
    );
}

#[test]
fn ac6_a_far_reset_capacity_wait_ends_at_the_reset_by_default() {
    let got = plan_capacity(NOW, Some(NOW + EIGHT_HOURS), None, None);
    assert_eq!(
        (got.until, got.bound, got.kind),
        (
            Some(NOW + EIGHT_HOURS + MARGIN),
            Bound::ResetMargin,
            PlanKind::Capacity
        )
    );
    let got = plan_capacity(NOW, Some(NOW + EIGHT_HOURS), Some(600), None);
    assert_eq!((got.until, got.bound), (Some(NOW + 600), Bound::MaxWait));
    let got = plan_capacity(NOW, Some(NOW + EIGHT_HOURS), None, Some((NOW, 3600)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 3540), Bound::StepTimeout)
    );
}

#[test]
fn ac6_a_capacity_wait_with_nothing_set_is_unbounded() {
    let got = plan_capacity(NOW, None, None, None);
    assert_eq!(
        (got.until, got.bound, got.kind, got.reset),
        (None, Bound::Unbounded, PlanKind::Capacity, None)
    );
    // A reset already past plays no part, so the wait is still unbounded.
    let got = plan_capacity(NOW, Some(NOW - 10), None, None);
    assert_eq!((got.until, got.bound), (None, Bound::Unbounded));
    // A step timeout of 0 does not apply.
    let got = plan_capacity(NOW, None, None, Some((NOW, 0)));
    assert_eq!((got.until, got.bound), (None, Bound::Unbounded));
}

#[test]
fn the_unbounded_bound_has_a_fixed_log_name() {
    assert_eq!(Bound::Unbounded.as_str(), "unbounded");
    assert_eq!(Bound::ResetMargin.as_str(), "reset+margin");
    assert_eq!(Bound::MaxWait.as_str(), "max_wait");
    assert_eq!(Bound::StepTimeout.as_str(), "step_timeout");
}

#[test]
fn no_skip_reason_says_reset_too_far() {
    for why in [
        NoWait::NoLimitMessage,
        NoWait::SpendLimit,
        NoWait::NoReset,
        NoWait::PipedPrompt,
    ] {
        assert_ne!(why.as_str(), "reset_too_far");
    }
}

// --- upgrade (issues #1801, #2022) ----------------------------------------

const FAR: i64 = NOW + 86_400;
const SEEN_FAR: Option<(Harness, Option<i64>)> = Some((Harness::Claude, Some(FAR)));
const SEEN_NONE: Option<(Harness, Option<i64>)> = Some((Harness::Claude, None));

fn far_reset() -> WaitFor {
    WaitFor::Reset {
        harness: Harness::Claude,
        reset: FAR,
    }
}

fn near_reset() -> WaitFor {
    WaitFor::Reset {
        harness: Harness::Claude,
        reset: NOW + 1800,
    }
}

#[test]
fn a_far_reset_becomes_a_capacity_wait_when_a_login_could_take_over() {
    assert_eq!(
        upgrade(far_reset(), SEEN_FAR, NOW, || true),
        WaitFor::Capacity {
            harness: Harness::Claude,
            reset: Some(FAR)
        }
    );
}

#[test]
fn an_unreadable_reset_becomes_a_capacity_wait_when_a_login_could_take_over() {
    assert_eq!(
        upgrade(WaitFor::Skip(NoWait::NoReset), SEEN_NONE, NOW, || true),
        WaitFor::Capacity {
            harness: Harness::Claude,
            reset: None
        }
    );
}

#[test]
fn a_far_reset_with_no_login_to_take_over_stays_a_reset_wait() {
    assert_eq!(upgrade(far_reset(), SEEN_FAR, NOW, || false), far_reset());
    assert_eq!(
        upgrade(WaitFor::Skip(NoWait::NoReset), SEEN_FAR, NOW, || false),
        WaitFor::Skip(NoWait::NoReset)
    );
}

// S1: a spend limit reports nothing as seen. It never becomes a capacity
// wait, so no login in another organisation is probed or switched to.
#[test]
fn a_spend_limit_is_never_made_a_capacity_wait() {
    assert_eq!(upgrade(far_reset(), None, NOW, || true), far_reset());
    let spend_weekly = wait_for(SPEND_WEEKLY.as_bytes(), b"", NOW);
    assert_eq!(upgrade(spend_weekly, None, NOW, || true), spend_weekly);
    assert!(matches!(spend_weekly, WaitFor::Reset { .. }));
}

#[test]
fn a_near_reset_wait_is_never_upgraded() {
    assert_eq!(upgrade(near_reset(), SEEN_FAR, NOW, || true), near_reset());
    let edge = WaitFor::Reset {
        harness: Harness::Claude,
        reset: NOW + FIVE_HOURS,
    };
    assert_eq!(
        upgrade(edge, SEEN_FAR, NOW, || true),
        edge,
        "exactly five hours is near"
    );
    let just_far = WaitFor::Reset {
        harness: Harness::Claude,
        reset: NOW + FIVE_HOURS + 1,
    };
    assert!(matches!(
        upgrade(just_far, SEEN_FAR, NOW, || true),
        WaitFor::Capacity { .. }
    ));
}

#[test]
fn only_a_far_or_unreadable_reset_is_upgraded() {
    for why in [
        NoWait::NoLimitMessage,
        NoWait::SpendLimit,
        NoWait::PipedPrompt,
    ] {
        assert_eq!(
            upgrade(WaitFor::Skip(why), SEEN_FAR, NOW, || true),
            WaitFor::Skip(why),
            "{why:?}"
        );
    }
}

#[test]
fn the_takeover_check_runs_only_for_a_limit_that_qualifies() {
    let calls = std::cell::Cell::new(0);
    let check = || {
        calls.set(calls.get() + 1);
        true
    };
    upgrade(near_reset(), SEEN_FAR, NOW, check);
    upgrade(far_reset(), None, NOW, check);
    upgrade(WaitFor::Skip(NoWait::PipedPrompt), SEEN_FAR, NOW, check);
    assert_eq!(
        calls.get(),
        0,
        "no store read for a limit that cannot qualify"
    );
    upgrade(far_reset(), SEEN_FAR, NOW, check);
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
            Some(NOW + 21_600),
            Bound::MaxWait,
            PlanKind::Capacity,
            Some(NOW + 86_400)
        )
    );
    let got = plan_capacity(NOW, None, Some(86_400), None);
    assert_eq!((got.until, got.reset), (Some(NOW + 86_400), None));
}

#[test]
fn a_step_timeout_ends_a_capacity_wait_sixty_seconds_before_it() {
    let got = plan_capacity(NOW, None, Some(21_600), Some((NOW - 100, 1000)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 840), Bound::StepTimeout)
    );
    let got = plan_capacity(NOW, None, Some(21_600), Some((NOW, 0)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 21_600), Bound::MaxWait),
        "a timeout of 0 does not apply"
    );
    let got = plan_capacity(NOW, None, Some(21_600), Some((NOW - 5000, 1000)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW), Bound::StepTimeout),
        "used up: never earlier than now"
    );
}

#[test]
fn a_tie_on_a_capacity_wait_is_named_max_wait() {
    let got = plan_capacity(NOW, None, Some(600), Some((NOW, 660)));
    assert_eq!((got.until, got.bound), (Some(NOW + 600), Bound::MaxWait));
}

#[test]
fn a_zero_maximum_is_an_immediate_capacity_deadline() {
    let got = plan_capacity(NOW, None, Some(0), None);
    assert_eq!((got.until, got.bound), (Some(NOW), Bound::MaxWait));
}

#[test]
fn extreme_capacity_inputs_saturate_without_panicking() {
    let got = plan_capacity(NOW, None, Some(u64::MAX), None);
    assert_eq!(got.until, Some(i64::MAX), "no overflow, no ceiling");
    let got = plan_capacity(NOW, None, Some(u64::MAX), Some((NOW, 1000)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 940), Bound::StepTimeout)
    );
    let got = plan_capacity(
        i64::MAX - 10,
        Some(i64::MAX),
        Some(u64::MAX),
        Some((i64::MAX, u64::MAX)),
    );
    assert!(got.until.is_some_and(|u| u >= i64::MAX - 10));
    let got = plan_capacity(i64::MIN, None, Some(0), Some((i64::MIN, 1)));
    assert_eq!(got.until, Some(i64::MIN), "never earlier than now");
}

// Issue #2022: the wait cap has no schema default; it is an opt-in ceiling.
#[test]
fn the_wait_cap_has_no_schema_default() {
    let key = crate::user_config::schema::KEYS
        .iter()
        .find(|k| k.name == super::settings::MAX_WAIT_KEY)
        .expect("the wait cap is a config key");
    assert_eq!(key.default, None);
}

// Issue #2022 (replaces #1801 AC5): with the setting unset a capacity wait
// has no length of its own; a step timeout or the failed login's reset ends it.
#[test]
fn an_unset_maximum_gives_no_capacity_ceiling() {
    let got = plan_capacity(NOW, None, None, None);
    assert_eq!(
        (got.until, got.bound, got.kind),
        (None, Bound::Unbounded, PlanKind::Capacity)
    );
    let got = plan_capacity(NOW, Some(NOW + 3 * 86_400), None, Some((NOW, 3600)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 3540), Bound::StepTimeout)
    );
    let got = plan_capacity(NOW, None, None, Some((NOW, 30_000)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 29_940), Bound::StepTimeout),
        "no six-hour default ends the wait before the step timeout"
    );
}

// With no maximum a reset wait ends at its reset plus margin.
#[test]
fn an_unset_maximum_does_not_bound_a_reset_wait() {
    let got = p(NOW + 4 * 3600, None, None);
    assert_eq!(
        (got.until, got.bound, got.kind),
        (
            Some(NOW + 4 * 3600 + MARGIN),
            Bound::ResetMargin,
            PlanKind::Reset
        )
    );
}

// V1: one second of maximum is one second of wait, and the largest value
// the schema accepts saturates rather than wrapping below now.
#[test]
fn capacity_maximum_boundaries() {
    assert_eq!(plan_capacity(NOW, None, Some(1), None).until, Some(NOW + 1));
    let big = u64::try_from(i64::MAX).unwrap();
    for max in [big - 1, big, big + 1] {
        let got = plan_capacity(NOW, None, Some(max), None);
        assert!(got.until.is_some_and(|u| u >= NOW), "max {max}: {got:?}");
        assert_eq!(got.bound, Bound::MaxWait);
    }
}

// R1: the failed login's own reset ends a capacity wait when it comes before
// the maximum, so a reset between five hours and the maximum is retried at
// the reset rather than failing at the maximum.
#[test]
fn a_capacity_wait_ends_at_the_failed_logins_reset_before_the_maximum() {
    let reset = NOW + 5 * 3600 + 1800;
    let got = plan_capacity(NOW, Some(reset), None, None);
    assert_eq!(
        (got.until, got.bound, got.kind, got.reset),
        (
            Some(reset + MARGIN),
            Bound::ResetMargin,
            PlanKind::Capacity,
            Some(reset)
        )
    );
    let got = plan_capacity(NOW, Some(NOW + 21_600 - MARGIN), Some(21_600), None);
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 21_600), Bound::ResetMargin),
        "a tie with the maximum is named by the reset, which retries"
    );
    let got = plan_capacity(NOW, Some(reset), None, Some((NOW, 3600)));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 3540), Bound::StepTimeout),
        "a step timeout that ends sooner still ends it"
    );
    let got = plan_capacity(NOW, Some(NOW - 10), Some(600), None);
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 600), Bound::MaxWait),
        "a reset already past plays no part"
    );
}
