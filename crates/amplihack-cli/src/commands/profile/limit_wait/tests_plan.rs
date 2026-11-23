//! `wait_for_message` and the deadline table (issues #1618, #2911).
//!
//! Two halves:
//!
//! - **which wait a limit earns.** Every limit this module can read waits.
//!   A reset the step can use inside five hours is waited for directly; a
//!   reset too far away, a reset already behind the clock, a message with no
//!   readable reset and Codex's credits each become a capacity wait naming
//!   its [`Waiting`]; a spend limit stating no reset becomes a login-change
//!   wait. The only two readings left that end a step are not limits: output
//!   with no limit message in it, and a prompt that could not be read to
//!   its end.
//! - **the deadline.** The earlier of the reset row and the wait's length
//!   wins, a tie is named by the reset row, and no reset row can push a
//!   reset wait past now + 5 h + 120 s. Every plan a limit produces ends
//!   strictly after `now`: that is the pacing rule that replaced #2731's
//!   per-step budget. There is no step-timeout row (issue #3037).
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_plan;`.

use super::plan::{
    Bound, CAPACITY_DEFAULT_MAX, MIN_WAIT_SECS, NoWait, Plan, PlanKind, WaitFor, Waiting, plan,
    plan_capacity, plan_login_change, rate_limit_capacity, wait_for_message,
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
const CREDITS: &str = "Your workspace is out of credits. Add credits to continue.";

fn wait_for(stderr: &[u8], stdout: &[u8], now: i64) -> WaitFor {
    wait_for_message(limit_patterns::limit_message(stderr, stdout, now), now)
}

fn p(reset: i64, max: Option<u64>) -> Plan {
    plan(NOW, reset, max)
}

fn reset_wait(reset: i64) -> WaitFor {
    WaitFor::Reset {
        harness: Harness::Claude,
        reset,
    }
}

fn capacity_wait(reset: Option<i64>, why: Waiting) -> WaitFor {
    WaitFor::Capacity {
        harness: Harness::Claude,
        reset,
        why,
    }
}

// --- which wait a limit earns --------------------------------------------

#[test]
fn a_session_limit_within_five_hours_is_waited_for() {
    assert_eq!(wait_for(SESSION.as_bytes(), b"", NOW), reset_wait(LA_1300));
}

#[test]
fn issue_2911_a_spend_limit_with_no_reset_waits_for_the_login_to_change() {
    assert_eq!(
        wait_for(SPEND.as_bytes(), b"", NOW),
        WaitFor::LoginChange {
            harness: Harness::Claude
        },
        "no usage window's reset lifts a spend limit"
    );
}

#[test]
fn a_spend_limit_that_states_a_near_reset_waits_that_window_out() {
    assert_eq!(
        wait_for(SPEND_SESSION.as_bytes(), b"", NOW),
        reset_wait(LA_1300),
        "the session window is what stops work first"
    );
}

#[test]
fn issue_2911_a_spend_limit_stating_a_far_reset_waits_for_capacity() {
    assert_eq!(
        wait_for(SPEND_WEEKLY.as_bytes(), b"", NOW),
        capacity_wait(
            Some(
                limit_patterns::limit_message(SPEND_WEEKLY.as_bytes(), b"", NOW)
                    .and_then(|m| m.reset)
                    .expect("the line states a weekly reset")
            ),
            Waiting::ResetTooFar
        )
    );
}

#[test]
fn issue_2911_a_reset_beyond_five_hours_waits_for_capacity() {
    // Before #2911 this was `Skip(ResetTooFar)`, which over 72 hours was the
    // reason 217 waits were refused outright and their runs ended.
    let weekly = limit_patterns::limit_message(WEEKLY.as_bytes(), b"", NOW)
        .and_then(|m| m.reset)
        .expect("the line states a weekly reset");
    assert_eq!(
        wait_for(WEEKLY.as_bytes(), b"", NOW),
        capacity_wait(Some(weekly), Waiting::ResetTooFar)
    );
    // One second past the cap.
    assert_eq!(
        wait_for(SESSION.as_bytes(), b"", NOW - 1),
        capacity_wait(Some(LA_1300), Waiting::ResetTooFar)
    );
}

#[test]
fn issue_2911_a_message_with_no_readable_reset_waits_for_capacity() {
    for msg in [
        "You've hit your session limit · try again later",
        "You've hit your weekly limit · resets Oct 8 (America/Los_Angeles)",
        "Claude AI usage limit reached|soon",
    ] {
        assert_eq!(
            wait_for(msg.as_bytes(), b"", NOW),
            capacity_wait(None, Waiting::NoReset),
            "{msg}"
        );
    }
}

#[test]
fn issue_2911_a_reset_already_behind_the_clock_waits_for_capacity() {
    // 13:02 local: the session limit's "1pm" is two minutes ago, inside the
    // ten minutes a bare clock time is still read as today's. A reset wait
    // for it would have a deadline of `now`, which is the zero-length wait
    // the pacing rule exists to prevent.
    let late = LA_1300 + 120;
    assert_eq!(
        wait_for(SESSION.as_bytes(), b"", late),
        capacity_wait(Some(LA_1300), Waiting::PassedReset)
    );
}

#[test]
fn issue_2911_codex_credits_wait_for_capacity() {
    assert_eq!(
        wait_for(CREDITS.as_bytes(), b"", NOW),
        WaitFor::Capacity {
            harness: Harness::Codex,
            reset: None,
            why: Waiting::Credits,
        },
        "Codex states no reset, so there is nothing to sleep to"
    );
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

/// The whole of what is left of [`NoWait`]: two readings, neither of them a
/// limit. Issue #2911's first invariant is that no limit can end a step, and
/// this is the enum that would have to grow for one to. The second is a
/// lost prompt, which fails a step whether or not it met a limit (review of
/// PR #2992); `piped_prompt`, which also covered a copy that failed or
/// could not be set up, is gone.
#[test]
fn issue_2911_only_two_readings_end_a_step_and_neither_is_a_limit() {
    assert_eq!(
        [
            NoWait::NoLimitMessage.as_str(),
            NoWait::PromptCutShort.as_str()
        ],
        ["no_limit_message", "prompt_cut_short"]
    );
    for text in [SESSION, SPEND, SPEND_SESSION, SPEND_WEEKLY, WEEKLY, CREDITS] {
        for now in [NOW, LA_1300 + 120, NOW + 10 * 86_400] {
            let got = wait_for(text.as_bytes(), b"", now);
            assert!(
                !matches!(got, WaitFor::Skip(_)),
                "{text:?} at {now} gave {got:?}"
            );
        }
    }
}

/// The three tokens an operator greps the logs for survive the move out of
/// `Skip`: they now name why a *wait* has no reset, rather than why a step
/// ended.
#[test]
fn issue_2911_the_log_vocabulary_survives_the_move_out_of_skip() {
    assert_eq!(
        [
            Waiting::ResetTooFar.as_str(),
            Waiting::NoReset.as_str(),
            Waiting::SpendLimit.as_str(),
            Waiting::Credits.as_str(),
            Waiting::PassedReset.as_str(),
        ],
        [
            "reset_too_far",
            "no_reset",
            "spend_limit",
            "credits",
            "passed_reset"
        ]
    );
}

#[test]
fn a_wait_says_whether_it_has_a_reset_to_end_on() {
    assert_eq!(reset_wait(NOW + 60).waiting_for_capacity(), None);
    assert_eq!(
        WaitFor::Skip(NoWait::PromptCutShort).waiting_for_capacity(),
        None
    );
    assert_eq!(
        capacity_wait(None, Waiting::NoReset).waiting_for_capacity(),
        Some(Waiting::NoReset)
    );
    assert_eq!(
        WaitFor::LoginChange {
            harness: Harness::Claude
        }
        .waiting_for_capacity(),
        Some(Waiting::SpendLimit),
        "the token a reader greps for is still written"
    );
}

// --- rate_limit_capacity (issue #2857) -----------------------------------
//
// The one reading left that is not a limit message. A bare 429 states no
// reset and says nothing about an account's windows, so with no login that
// could take over it goes back to the runner as the transient fault the
// classifier calls it, and nothing is logged.

#[test]
fn a_bare_429_is_a_capacity_wait_when_a_login_could_take_over() {
    assert_eq!(
        rate_limit_capacity(Harness::Claude, || true),
        Some(capacity_wait(None, Waiting::NoReset))
    );
}

#[test]
fn a_bare_429_with_no_login_to_take_over_is_not_a_wait_at_all() {
    assert_eq!(rate_limit_capacity(Harness::Claude, || false), None);
}

#[test]
fn the_takeover_check_is_asked_once_and_only_here() {
    let calls = std::cell::Cell::new(0);
    let check = || {
        calls.set(calls.get() + 1);
        true
    };
    rate_limit_capacity(Harness::Claude, check);
    assert_eq!(calls.get(), 1);
    // And no limit message consults it: `wait_for_message` takes no such
    // argument, which is how #2911 removed the `reset_too_far` ending.
    assert_eq!(
        wait_for(WEEKLY.as_bytes(), b"", NOW).waiting_for_capacity(),
        Some(Waiting::ResetTooFar)
    );
    assert_eq!(calls.get(), 1, "a limit never asks");
}

// --- plan ----------------------------------------------------------------

#[test]
fn a_future_reset_waits_until_reset_plus_margin() {
    let got = p(NOW + 1800, None);
    assert_eq!(
        (got.until, got.bound, got.reset),
        (NOW + 1800 + MARGIN, Bound::ResetMargin, Some(NOW + 1800))
    );
}

#[test]
fn a_maximum_only_shortens_the_wait() {
    let got = p(NOW + 1800, Some(600));
    assert_eq!((got.until, got.bound), (NOW + 600, Bound::MaxWait));
    let got = p(NOW + 1800, Some(7200));
    assert_eq!((got.until, got.bound), (NOW + 1920, Bound::ResetMargin));
}

#[test]
fn a_tie_is_named_by_the_earlier_row() {
    let got = p(NOW + 480, Some(600));
    assert_eq!((got.until, got.bound), (NOW + 600, Bound::ResetMargin));
    let got = p(NOW + 1800, Some(600));
    assert_eq!((got.until, got.bound), (NOW + 600, Bound::MaxWait));
}

/// Issue #3037, review of PR #2992: a deadline is set by the limit's reset
/// or by the wait's own length, and by nothing else. The match is
/// exhaustive on purpose: a row added back for the step timeout, or for any
/// other clock, does not compile here until this test says what it is.
#[test]
fn issue_3037_a_deadline_is_the_reset_or_the_wait_length_and_nothing_else() {
    fn row(b: Bound) -> &'static str {
        match b {
            Bound::ResetMargin => "reset+margin",
            Bound::MaxWait => "max_wait",
        }
    }
    for b in [Bound::ResetMargin, Bound::MaxWait] {
        assert_eq!(b.as_str(), row(b));
    }
    // Each planner's deadline is exactly its own rows, however long the
    // step has already run: none of them is told when the step started.
    let got = p(NOW + 1800, None);
    assert_eq!(
        (got.until, got.bound),
        (NOW + 1800 + MARGIN, Bound::ResetMargin)
    );
    let got = plan_capacity(NOW, None, Some(21_600));
    assert_eq!((got.until, got.bound), (NOW + 21_600, Bound::MaxWait));
    let got = plan_login_change(NOW, Some(21_600));
    assert_eq!((got.until, got.bound), (NOW + 21_600, Bound::MaxWait));
}

#[test]
fn extreme_inputs_saturate_without_panicking() {
    let got = p(i64::MAX, Some(u64::MAX));
    assert_eq!(got.until, NOW + CEILING);
    let got = p(i64::MIN, None);
    assert_eq!(got.until, NOW + MARGIN);
    let got = plan(i64::MAX - 10, i64::MAX, Some(u64::MAX));
    assert!(got.until >= i64::MAX - 10);
    let got = plan(i64::MIN, i64::MIN, None);
    assert!(got.until > i64::MIN, "strictly after now");
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
    for r in resets {
        for m in maxes {
            let got = p(r, m);
            assert!(
                got.until >= NOW && got.until - NOW <= CEILING,
                "reset {r} max {m:?}"
            );
        }
    }
}

// --- the pacing rule (issue #2911) ---------------------------------------
//
// #2731 bounded a chain of limits with a per-step budget, which ended runs.
// What bounds it now is that every wait costs time: each plan a limit can
// produce ends strictly after `now`, so a chain is paced by waiting rather
// than by counting. Nothing brings a deadline back to `now`: the step
// timeout no longer has a row (issue #3037).

/// The floor the configured length is read through, which is what keeps the
/// rule true for every configured value and not only for an unset one. Two
/// poll intervals of the wait loop; `tests_settings.rs` holds the reading.
#[test]
fn issue_2911_the_floor_is_two_poll_intervals() {
    assert_eq!(MIN_WAIT_SECS, 60);
}

#[test]
fn issue_2911_every_plan_a_limit_produces_ends_strictly_after_now() {
    // The reset rows a limit can reach: `wait_for_message` only builds a
    // reset wait from a reset strictly ahead of `now` and inside five hours,
    // and every other limit is a capacity or login-change wait.
    let maxes = [None, Some(MIN_WAIT_SECS), Some(600), Some(21_600)];
    for max in maxes {
        for ahead in [1i64, 60, 1800, FIVE_HOURS] {
            let got = plan(NOW, NOW + ahead, max);
            assert!(
                got.until > NOW,
                "reset {ahead}s ahead, max {max:?}: {got:?}"
            );
        }
        for reset in [None, Some(NOW - 86_400), Some(NOW + 7 * 86_400)] {
            let got = plan_capacity(NOW, reset, max);
            assert!(got.until > NOW, "capacity, reset {reset:?}, max {max:?}");
        }
        let got = plan_login_change(NOW, max);
        assert!(got.until > NOW, "login change, max {max:?}");
    }
}

/// A reset already behind the clock never reaches [`plan`]: it is a capacity
/// wait before it gets there. The floor here is the backstop, so that a
/// caller this file cannot see still cannot plan a wait of no length.
#[test]
fn a_reset_at_or_before_now_is_still_floored_at_the_margin() {
    for reset in [NOW, NOW - 300, i64::MIN] {
        let got = p(reset, None);
        assert_eq!(
            (got.until, got.bound, got.reset),
            (NOW + MARGIN, Bound::ResetMargin, Some(reset))
        );
    }
}

// --- plan_capacity (issue #1801) -----------------------------------------

#[test]
fn a_reset_plan_is_of_the_reset_kind() {
    assert_eq!(p(NOW + 1800, None).kind, PlanKind::Reset);
}

#[test]
fn a_capacity_wait_lasts_the_maximum_with_no_five_hour_ceiling() {
    let got = plan_capacity(NOW, Some(NOW + 86_400), Some(21_600));
    assert_eq!(
        (got.until, got.bound, got.kind, got.reset),
        (
            NOW + 21_600,
            Bound::MaxWait,
            PlanKind::Capacity,
            Some(NOW + 86_400)
        )
    );
    let got = plan_capacity(NOW, None, Some(86_400));
    assert_eq!((got.until, got.reset), (NOW + 86_400, None));
}

#[test]
fn extreme_capacity_inputs_saturate_without_panicking() {
    let got = plan_capacity(NOW, None, Some(u64::MAX));
    assert_eq!(got.until, i64::MAX, "no overflow, no ceiling");
    let got = plan_capacity(i64::MAX - 10, Some(i64::MAX), Some(u64::MAX));
    assert!(got.until >= i64::MAX - 10);
    let got = plan_capacity(i64::MIN, None, Some(MIN_WAIT_SECS));
    assert!(got.until > i64::MIN, "strictly after now");
}

#[test]
fn the_capacity_default_is_the_schema_default() {
    let key = crate::user_config::schema::KEYS
        .iter()
        .find(|k| k.name == super::settings::MAX_WAIT_KEY)
        .expect("the wait cap is a config key");
    assert_eq!(key.default, Some(CAPACITY_DEFAULT_MAX.to_string().as_str()));
}

// AC5: with the setting unset, a capacity wait lasts the six-hour default.
// A reset further away does not lengthen it: the wait ends at the default
// and the step retries.
#[test]
fn ac5_an_unset_maximum_gives_a_capacity_wait_of_six_hours() {
    let got = plan_capacity(NOW, None, None);
    assert_eq!(
        (got.until, got.bound, got.kind),
        (NOW + 21_600, Bound::MaxWait, PlanKind::Capacity)
    );
    let got = plan_capacity(NOW, Some(NOW + 3 * 86_400), None);
    assert_eq!((got.until, got.bound), (NOW + 21_600, Bound::MaxWait));
}

// The six-hour capacity default never reaches a reset wait: with no maximum
// a reset wait still ends at its reset plus margin, inside five hours.
#[test]
fn the_capacity_default_does_not_bound_a_reset_wait() {
    let got = p(NOW + 4 * 3600, None);
    assert_eq!(
        (got.until, got.bound, got.kind),
        (NOW + 4 * 3600 + MARGIN, Bound::ResetMargin, PlanKind::Reset)
    );
}

// V1: the shortest length the setting can ask for is one wait of that
// length, and the largest value the schema accepts saturates rather than
// wrapping below now.
#[test]
fn capacity_maximum_boundaries() {
    assert_eq!(
        plan_capacity(NOW, None, Some(MIN_WAIT_SECS)).until,
        NOW + i64::try_from(MIN_WAIT_SECS).unwrap()
    );
    let big = u64::try_from(i64::MAX).unwrap();
    for max in [big - 1, big, big + 1] {
        let got = plan_capacity(NOW, None, Some(max));
        assert!(got.until >= NOW, "max {max}: {got:?}");
        assert_eq!(got.bound, Bound::MaxWait);
    }
}

// R1: the failed login's own reset ends a capacity wait when it comes before
// the maximum, so a reset between five hours and the maximum is retried at
// the reset rather than failing at the maximum.
#[test]
fn a_capacity_wait_ends_at_the_failed_logins_reset_before_the_maximum() {
    let reset = NOW + 5 * 3600 + 1800;
    let got = plan_capacity(NOW, Some(reset), None);
    assert_eq!(
        (got.until, got.bound, got.kind, got.reset),
        (
            reset + MARGIN,
            Bound::ResetMargin,
            PlanKind::Capacity,
            Some(reset)
        )
    );
    let got = plan_capacity(NOW, Some(NOW + 21_600 - MARGIN), Some(21_600));
    assert_eq!(
        (got.until, got.bound),
        (NOW + 21_600, Bound::ResetMargin),
        "a tie with the maximum is named by the reset, which retries"
    );
    let got = plan_capacity(NOW, Some(NOW - 10), Some(600));
    assert_eq!(
        (got.until, got.bound),
        (NOW + 600, Bound::MaxWait),
        "a reset already past plays no part"
    );
}

/// Why a passed reset is a capacity wait and not a reset wait: the plan the
/// capacity route gives has real length, so a chain of limits is paced.
#[test]
fn issue_2911_the_wait_for_a_passed_reset_has_real_length() {
    let past = NOW - 120;
    let WaitFor::Capacity { reset, why, .. } = wait_for(SESSION.as_bytes(), b"", LA_1300 + 120)
    else {
        panic!("a passed reset is a capacity wait");
    };
    assert_eq!(why, Waiting::PassedReset);
    for max in [Some(MIN_WAIT_SECS), Some(600), Some(21_600), None] {
        let got = plan_capacity(NOW, reset, max);
        assert!(got.until > NOW, "max {max:?}: {got:?}");
    }
    // And the plan a reset wait would have given, had the planner built one:
    // the margin is what keeps even that above `now`.
    assert_eq!(plan(NOW, past, None).until, NOW + MARGIN);
}
