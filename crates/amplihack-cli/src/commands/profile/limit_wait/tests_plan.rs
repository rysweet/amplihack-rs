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
//! - **the deadline.** A reset wait ends at its reset plus the margin and
//!   has no other row: no configured length and no five-hour ceiling. A
//!   capacity wait whose limit stated a reset still ahead ends at that
//!   reset, held to its kind's window. A capacity wait with no reset ahead,
//!   and a login-change wait, have no deadline: they end only on an event
//!   (issue #2911). Every plan with a deadline ends strictly after `now`,
//!   and no plan has a timed row of any other kind: that is the pacing rule
//!   that replaced #2731's per-step budget. There is no step-timeout row
//!   (issue #3037).
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_plan;`.

use super::plan::{
    Bound, NoWait, Plan, PlanKind, WaitFor, Waiting, plan, plan_capacity, plan_login_change,
    rate_limit_capacity, wait_for_message,
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
const CREDITS: &str = "Your workspace is out of credits. Add credits to continue.";
/// A weekly reset inside the seven-day window: 2026-10-22 00:00 -07:00.
const WEEKLY_IN_4_DAYS: &str =
    "You've hit your weekly limit · resets Oct 22, 12am (America/Los_Angeles)";
const LA_OCT_22: i64 = 1_792_652_400;
const SEVEN_DAYS: i64 = 7 * 86_400;

fn wait_for(stderr: &[u8], stdout: &[u8], now: i64) -> WaitFor {
    wait_for_message(limit_patterns::limit_message(stderr, stdout, now), now)
}

fn p(reset: i64) -> Plan {
    plan(NOW, reset)
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

/// A spend limit has no fixed window, so its stated reset is held to the
/// rotate ledger's span rule, 62 days, and no further.
#[test]
fn issue_2911_a_spend_limit_stating_a_far_reset_waits_for_capacity() {
    let stated = limit_patterns::limit_message(SPEND_WEEKLY.as_bytes(), b"", NOW)
        .and_then(|m| m.reset)
        .expect("the line states a weekly reset");
    assert_eq!(
        wait_for(SPEND_WEEKLY.as_bytes(), b"", NOW),
        capacity_wait(Some(stated.min(NOW + 62 * 86_400)), Waiting::ResetTooFar)
    );
}

#[test]
fn issue_2911_a_reset_beyond_five_hours_waits_for_capacity() {
    // Before #2911 this was `Skip(ResetTooFar)`, which over 72 hours was the
    // reason 217 waits were refused outright and their runs ended.
    assert_eq!(
        wait_for(WEEKLY_IN_4_DAYS.as_bytes(), b"", NOW),
        capacity_wait(Some(LA_OCT_22), Waiting::ResetTooFar),
        "a genuine weekly reset is waited for as stated"
    );
    // One second past the cap: a capacity wait, whose reset is held to the
    // session window from now, one second short of the stated 13:00.
    assert_eq!(
        wait_for(SESSION.as_bytes(), b"", NOW - 1),
        capacity_wait(Some(LA_1300 - 1), Waiting::ResetTooFar)
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
/// lost prompt, which fails a step whether or not it met a limit;
/// `piped_prompt`, which also covered a copy that failed or
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
    let got = p(NOW + 1800);
    assert_eq!(
        (got.until, got.bound, got.reset),
        (
            Some(NOW + 1800 + MARGIN),
            Bound::ResetMargin,
            Some(NOW + 1800)
        )
    );
}

/// A reset wait has no timed bound. Its reset is the limit event it waits
/// for, so its deadline is that reset plus the margin for every reset the
/// router can hand it. `plan` takes no length, so no setting can reach it.
#[test]
fn a_reset_wait_ends_at_its_reset_and_on_no_timer() {
    for ahead in [1i64, 60, 61, 600, 1800, 7200, FIVE_HOURS] {
        let got = p(NOW + ahead);
        assert_eq!(
            (got.until, got.bound, got.kind),
            (
                Some(NOW + ahead + MARGIN),
                Bound::ResetMargin,
                PlanKind::Reset
            ),
            "a reset {ahead}s ahead"
        );
    }
}

/// There is no five-hour ceiling on a reset wait. The five-hour rule decides
/// which wait a limit earns, once, in the router, and a reset beyond it is a
/// capacity wait before it gets here.
#[test]
fn a_reset_wait_has_no_five_hour_ceiling() {
    let far = NOW + FIVE_HOURS + 3600;
    let got = p(far);
    assert_eq!(
        (got.until, got.bound),
        (Some(far + MARGIN), Bound::ResetMargin),
        "a reset the router would never send here is still waited for as stated"
    );
    assert!(
        matches!(
            wait_for(WEEKLY_IN_4_DAYS.as_bytes(), b"", NOW),
            WaitFor::Capacity { reset: Some(r), why: Waiting::ResetTooFar, .. } if r == LA_OCT_22
        ),
        "a weekly reset is a capacity wait, so `plan` never sees it"
    );
}

/// A stated reset is never trusted further than its kind's window: a weekly
/// limit whose message puts the reset almost a year away (this fixture's Oct
/// 8, read on Oct 18) is held to seven days, and a session limit to five
/// hours. The step cannot tell the harness's message from a model that
/// printed one, so this is what bounds how long a forged message holds it.
#[test]
fn a_stated_reset_is_held_to_its_kinds_window() {
    let stated = limit_patterns::limit_message(WEEKLY.as_bytes(), b"", NOW)
        .and_then(|m| m.reset)
        .expect("the line states a weekly reset");
    assert!(
        stated > NOW + SEVEN_DAYS,
        "the fixture's reset is past the window"
    );
    assert_eq!(
        wait_for(WEEKLY.as_bytes(), b"", NOW),
        capacity_wait(Some(NOW + SEVEN_DAYS), Waiting::ResetTooFar)
    );
    assert_eq!(
        super::plan::held(NOW + 86_400, NOW, Some(limit_patterns::LimitKind::Session)),
        NOW + FIVE_HOURS
    );
    assert_eq!(
        super::plan::held(NOW + 86_400, NOW, Some(limit_patterns::LimitKind::Weekly)),
        NOW + 86_400,
        "a genuine reset inside the window is not shortened"
    );
    assert_eq!(
        super::plan::held(NOW + 30 * 86_400, NOW, None),
        NOW + SEVEN_DAYS
    );
}

/// Issues #3037 and #2911: a deadline is set by a limit's reset and by
/// nothing else, and a wait with no usable reset has none. The match is
/// exhaustive on purpose: a row added back for the step timeout, for a
/// configured length or for any other clock does not compile here until
/// this test says what it is.
///
/// Issue #2967 adds one row, and it is a reset too: another login the step
/// could move to recovering, read from that login's reading
/// ([`super::plan::earliest`]). It is no clock of the wait's own.
#[test]
fn a_deadline_is_the_reset_or_there_is_none() {
    fn row(b: Bound) -> &'static str {
        match b {
            Bound::ResetMargin => "reset+margin",
            Bound::EarliestReset => "earliest_reset+margin",
            Bound::Unbounded => "none",
        }
    }
    for b in [Bound::ResetMargin, Bound::EarliestReset, Bound::Unbounded] {
        assert_eq!(b.as_str(), row(b));
    }
    // Each planner's deadline is exactly its own rows, however long the
    // step has already run: none of them is told when the step started.
    let got = p(NOW + 1800);
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 1800 + MARGIN), Bound::ResetMargin)
    );
    let got = plan_capacity(NOW, None);
    assert_eq!((got.until, got.bound), (None, Bound::Unbounded));
    let got = plan_login_change();
    assert_eq!((got.until, got.bound), (None, Bound::Unbounded));
}

#[test]
fn extreme_inputs_saturate_without_panicking() {
    let got = p(i64::MAX);
    assert_eq!(got.until, Some(i64::MAX), "saturates rather than wrapping");
    let got = p(i64::MIN);
    assert_eq!(got.until, Some(NOW + MARGIN));
    let got = plan(i64::MAX - 10, i64::MAX);
    assert!(got.until >= Some(i64::MAX - 10));
    let got = plan(i64::MIN, i64::MIN);
    assert!(got.until > Some(i64::MIN), "strictly after now");
}

/// A reset wait's deadline is the later of its reset and `now`, plus the
/// margin: never before `now + MARGIN`, and never cut short.
#[test]
fn until_is_the_reset_plus_margin_and_never_before_now_plus_margin() {
    let resets = [
        i64::MIN,
        NOW - 1,
        NOW,
        NOW + 1,
        NOW + FIVE_HOURS,
        NOW + FIVE_HOURS + 1,
        i64::MAX,
    ];
    for r in resets {
        let got = p(r);
        assert_eq!(
            got.until,
            Some(r.max(NOW).saturating_add(MARGIN)),
            "reset {r}"
        );
        assert!(got.until >= Some(NOW + MARGIN), "reset {r}");
    }
}

// --- the pacing rule (issue #2911) ---------------------------------------
//
// #2731 bounded a chain of limits with a per-step budget, which ended runs.
// What paces a chain now is that every wait lasts until something changed:
// a plan with a deadline ends strictly after `now`, and a plan without one
// ends only on an event. Nothing brings a deadline back to `now`, and no
// plan has a timed row that would run the step again on a login nothing
// has shown to be usable.

#[test]
fn issue_2911_every_plan_a_limit_produces_ends_after_now_or_only_on_an_event() {
    // The reset rows a limit can reach: `wait_for_message` only builds a
    // reset wait from a reset strictly ahead of `now` and inside five hours,
    // and every other limit is a capacity or login-change wait.
    for ahead in [1i64, 60, 1800, FIVE_HOURS] {
        let got = plan(NOW, NOW + ahead);
        assert!(got.until > Some(NOW), "reset {ahead}s ahead: {got:?}");
    }
    let got = plan_capacity(NOW, Some(NOW + 7 * 86_400));
    assert!(got.until > Some(NOW), "{got:?}");
    for reset in [None, Some(NOW), Some(NOW - 86_400)] {
        let got = plan_capacity(NOW, reset);
        assert_eq!(
            (got.until, got.bound),
            (None, Bound::Unbounded),
            "capacity, reset {reset:?}: no deadline, so no timer runs the step again"
        );
    }
    let got = plan_login_change();
    assert_eq!((got.until, got.bound), (None, Bound::Unbounded));
    assert!(got.is_login_change());
}

/// A reset already behind the clock never reaches [`plan`]: it is a capacity
/// wait before it gets there. The floor here is the backstop, so that a
/// caller this file cannot see still cannot plan a wait of no length.
#[test]
fn a_reset_at_or_before_now_is_still_floored_at_the_margin() {
    for reset in [NOW, NOW - 300, i64::MIN] {
        let got = p(reset);
        assert_eq!(
            (got.until, got.bound, got.reset),
            (Some(NOW + MARGIN), Bound::ResetMargin, Some(reset))
        );
    }
}

// --- plan_capacity (issue #1801) -----------------------------------------

#[test]
fn a_reset_plan_is_of_the_reset_kind() {
    assert_eq!(p(NOW + 1800).kind, PlanKind::Reset);
}

/// A capacity wait whose limit stated a reset still ahead waits for that
/// reset, however far away. A weekly reset a day away is waited for once,
/// not re-asked on a timer.
#[test]
fn a_capacity_wait_with_a_reset_ahead_ends_at_that_reset_and_on_no_timer() {
    let got = plan_capacity(NOW, Some(NOW + 86_400));
    assert_eq!(
        (got.until, got.bound, got.kind, got.reset),
        (
            Some(NOW + 86_400 + MARGIN),
            Bound::ResetMargin,
            PlanKind::Capacity,
            Some(NOW + 86_400)
        )
    );
}

#[test]
fn extreme_capacity_inputs_saturate_without_panicking() {
    let got = plan_capacity(i64::MAX - 10, Some(i64::MAX));
    assert!(got.until >= Some(i64::MAX - 10));
    let got = plan_capacity(i64::MIN, None);
    assert_eq!(got.until, None);
    let got = plan_capacity(i64::MIN, Some(i64::MIN + 1));
    assert!(got.until > Some(i64::MIN), "strictly after now");
}

/// Issue #2911: the setting that gave a wait with no reset its length is
/// gone from the schema, so nothing can configure a timer back in.
#[test]
fn issue_2911_no_config_key_sets_a_wait_length() {
    assert!(
        crate::user_config::schema::KEYS
            .iter()
            .all(|k| k.name != "recipe.usage_limit_max_wait_secs"),
        "a wait has no length setting"
    );
}

/// A capacity wait with no reset ahead has no deadline. A reset ahead,
/// however far, is waited for instead.
#[test]
fn a_capacity_wait_with_no_reset_has_no_deadline() {
    let got = plan_capacity(NOW, None);
    assert_eq!(
        (got.until, got.bound, got.kind),
        (None, Bound::Unbounded, PlanKind::Capacity)
    );
    let got = plan_capacity(NOW, Some(NOW + 3 * 86_400));
    assert_eq!(
        (got.until, got.bound),
        (Some(NOW + 3 * 86_400 + MARGIN), Bound::ResetMargin)
    );
}

// R1: the failed login's own reset ends a capacity wait; a reset already
// past plays no part, and leaves the wait with no deadline.
#[test]
fn a_capacity_wait_ends_at_the_failed_logins_reset() {
    let reset = NOW + 5 * 3600 + 1800;
    let got = plan_capacity(NOW, Some(reset));
    assert_eq!(
        (got.until, got.bound, got.kind, got.reset),
        (
            Some(reset + MARGIN),
            Bound::ResetMargin,
            PlanKind::Capacity,
            Some(reset)
        )
    );
    let got = plan_capacity(NOW, Some(NOW - 10));
    assert_eq!(
        (got.until, got.bound, got.reset),
        (None, Bound::Unbounded, Some(NOW - 10)),
        "a reset already past plays no part in the deadline, and is still shown"
    );
}

/// Why a passed reset is a capacity wait and not a reset wait: a reset
/// wait for a time already gone would end at once and run the step again
/// straight into the same limit. The capacity route waits for an event
/// instead, so a chain of limits is paced.
#[test]
fn issue_2911_the_wait_for_a_passed_reset_waits_for_an_event() {
    let past = NOW - 120;
    let WaitFor::Capacity { reset, why, .. } = wait_for(SESSION.as_bytes(), b"", LA_1300 + 120)
    else {
        panic!("a passed reset is a capacity wait");
    };
    assert_eq!(why, Waiting::PassedReset);
    let got = plan_capacity(LA_1300 + 120, reset);
    assert_eq!((got.until, got.bound), (None, Bound::Unbounded));
    // And the plan a reset wait would have given, had the planner built one:
    // the margin is what keeps even that above `now`.
    assert_eq!(plan(NOW, past).until, Some(NOW + MARGIN));
}

// --- issue #2967: the earliest-reset deadline -------------------------------
//
// Contract: `earliest(p, other, now) -> Plan`. `other` is the earliest
// instant at which another login the step could move to becomes usable,
// on the wait's clock (`None` when there is none). It is held to the weekly
// window, as any reset read from a reading is, and it moves `p`'s deadline
// to `other + MARGIN`, named `Bound::EarliestReset`
// (`earliest_reset+margin`), when `other` is strictly after `now` and that
// deadline comes strictly before `p.until`, or `p` has none. The plan's
// kind, its `reset=` and its login-change flag are kept. A login-change
// wait is returned as it was. No input adds a deadline later than the one
// `p` had.

use super::plan::earliest;

const DAY: i64 = 86_400;

#[test]
fn the_bound_is_named_earliest_reset_plus_margin() {
    assert_eq!(Bound::EarliestReset.as_str(), "earliest_reset+margin");
}

#[test]
fn an_other_reset_before_the_own_one_moves_a_reset_waits_deadline() {
    let own = plan(NOW, NOW + 1800);
    let p = earliest(own, Some(NOW + 900), NOW);
    assert_eq!(p.until, Some(NOW + 900 + MARGIN));
    assert_eq!(p.bound, Bound::EarliestReset);
    assert_eq!(p.kind, PlanKind::Reset, "the kind of wait is kept");
    assert_eq!(p.reset, Some(NOW + 1800), "`reset=` stays the limit's own");
}

#[test]
fn an_other_reset_at_or_after_the_own_one_changes_nothing() {
    let own = plan(NOW, NOW + 1800);
    // The own deadline is the reset plus the margin, so an other reset at
    // the own reset ties and keeps the own row.
    assert_eq!(earliest(own, Some(NOW + 1800), NOW), own);
    assert_eq!(earliest(own, Some(NOW + 1801), NOW), own);
    assert_eq!(earliest(own, None, NOW), own);
}

#[test]
fn an_other_reset_at_or_before_now_is_ignored() {
    // A reset that has passed is a login that is usable, which the move
    // before the wait has already considered.
    for own in [plan(NOW, NOW + 1800), plan_capacity(NOW, None)] {
        assert_eq!(earliest(own, Some(NOW), NOW), own);
        assert_eq!(earliest(own, Some(NOW - 1), NOW), own);
        assert_eq!(earliest(own, Some(i64::MIN), NOW), own);
    }
}

#[test]
fn a_wait_with_no_deadline_takes_the_other_reset_as_its_deadline() {
    // A limit with no usable reset (none stated, credits, a passed reset,
    // a bare 429) waits with no deadline; another login recovering is a
    // limit event it can end on.
    for reset in [None, Some(NOW - 60)] {
        let own = plan_capacity(NOW, reset);
        assert_eq!(own.until, None);
        let p = earliest(own, Some(NOW + 1200), NOW);
        assert_eq!(p.until, Some(NOW + 1200 + MARGIN), "own reset {reset:?}");
        assert_eq!(p.bound, Bound::EarliestReset);
        assert_eq!(p.kind, PlanKind::Capacity, "still a capacity wait");
        assert_eq!(p.reset, reset);
    }
}

#[test]
fn a_far_capacity_wait_takes_a_sooner_other_reset() {
    let own = plan_capacity(NOW, Some(NOW + 3 * DAY));
    let p = earliest(own, Some(NOW + 1200), NOW);
    assert_eq!(p.until, Some(NOW + 1200 + MARGIN));
    assert_eq!(p.bound, Bound::EarliestReset);
    assert_eq!(p.kind, PlanKind::Capacity);
}

#[test]
fn a_capacity_wait_for_a_reset_ahead_and_sooner_than_the_other_is_unchanged() {
    let own = plan_capacity(NOW, Some(NOW + 600));
    assert_eq!(earliest(own, Some(NOW + 1200), NOW), own);
}

#[test]
fn an_other_reset_further_than_five_hours_still_counts_when_it_is_sooner() {
    // There is no five-hour ceiling on a wait's deadline (#2911); the other
    // login's reset counts as the own one would, held to the weekly window.
    let own = plan_capacity(NOW, Some(NOW + 3 * DAY));
    let p = earliest(own, Some(NOW + DAY), NOW);
    assert_eq!(p.until, Some(NOW + DAY + MARGIN));
}

#[test]
fn the_other_reset_is_held_to_the_weekly_window() {
    let own = plan_capacity(NOW, None);
    let p = earliest(own, Some(i64::MAX), NOW);
    assert_eq!(p.until, Some(NOW + SEVEN_DAYS + MARGIN));
}

#[test]
fn a_login_change_wait_is_returned_as_it_was() {
    // It waits for an organisation's credit, which no other login's window
    // restores (#2731).
    let own = plan_login_change();
    let p = earliest(own, Some(NOW + 600), NOW);
    assert_eq!(p, own);
    assert!(p.is_login_change());
}

#[test]
fn the_earliest_plan_is_not_a_login_change_wait_and_room_still_ends_it() {
    let p = earliest(plan(NOW, NOW + 1800), Some(NOW + 900), NOW);
    assert!(!p.is_login_change());
    assert!(p.capacity_may_end(), "a login with room still ends it");
}

#[test]
fn no_input_makes_a_deadline_later_than_the_one_the_plan_had() {
    for own in [
        plan(NOW, NOW + 1800),
        plan_capacity(NOW, Some(NOW + 3 * DAY)),
        plan_capacity(NOW, None),
    ] {
        for other in [None, Some(NOW + 1), Some(NOW + 5 * DAY), Some(i64::MAX)] {
            let p = earliest(own, other, NOW);
            if let Some(before) = own.until {
                assert!(p.until.is_some_and(|after| after <= before), "{other:?}");
            }
            assert!(p.until.is_none_or(|u| u > NOW), "{other:?}");
        }
    }
}

#[test]
fn the_earliest_plan_is_checked_arithmetic_at_the_extremes() {
    for now in [i64::MAX, i64::MIN, i64::MAX - 100, 0] {
        for other in [i64::MAX, i64::MIN, now, now.saturating_add(1)] {
            let _ = earliest(plan(now, now.saturating_add(10)), Some(other), now);
            let _ = earliest(plan_capacity(now, None), Some(other), now);
        }
    }
}
