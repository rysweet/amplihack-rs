//! Issue #2731: the wait's lines in their two sinks.
//!
//! The stderr line keeps its fields and order; a login-change wait's `start`
//! carries `wait=login_change` where a capacity wait carries `wait=capacity`.
//! The recipe-log `start` is that line with `harness=` dropped and
//! `account=` put in its place, and the recipe-log `end` is identical to the
//! stderr one, so one `grep` over both sinks reads as one stream.
//!
//! The `start` line's `reason` names the limit that started the wait rather
//! than the shape of the wait: `spend_limit` for an organisation spend limit
//! and `usage_limit` for every other, so a reader grepping the recipe log
//! for a spend limit finds the wait as well as the step's closing line
//! (issue #2731, the deputy's point 5).
//!
//! No other event reaches the recipe log: not `progress`, not `resume`, not
//! `skipped`, and not the closing `failed` line.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_log_recipe;`.

use super::Met;
use super::events::{Event, Shape, Skip, line, recipe_line};
use super::plan::{Bound, NoWait, Waiting};
use super::wait::End;
use crate::commands::recipe::run::limit_patterns::Harness;

/// 2026-10-18 08:00:00 -07:00.
const T0: i64 = 1_792_335_600;
const T30: i64 = T0 + 1800;
const T32: i64 = T0 + 1920;
const PREFIX: &str = "amplihack.recipe.usage_limit_wait ";

const LA: chrono_tz::Tz = chrono_tz::America::Los_Angeles;

fn start(shape: Shape, reset: Option<i64>, until: i64, bound: Bound) -> Event {
    Event::Start {
        harness: Harness::Claude,
        reset,
        until,
        bound,
        shape,
    }
}

fn stderr(e: &Event) -> String {
    line(e, &LA)
}

fn recipe(e: &Event, account: Option<&str>) -> Option<String> {
    recipe_line(e, account, &LA)
}

// --- the stderr start line ------------------------------------------------

#[test]
fn a_reset_waits_start_line_is_unchanged() {
    assert_eq!(
        stderr(&start(Shape::Reset, Some(T30), T32, Bound::ResetMargin)),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit harness=claude \
         reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 08:32:00 -07:00 bound=reset+margin"
    );
}

#[test]
fn a_capacity_waits_start_line_is_unchanged() {
    assert_eq!(
        stderr(&start(Shape::Capacity, None, T0, Bound::MaxWait)),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity \
         harness=claude reset=none until=2026-10-18 08:00:00 -07:00 bound=max_wait"
    );
}

#[test]
fn a_login_change_waits_start_line_says_so_in_the_same_place() {
    assert_eq!(
        stderr(&start(Shape::LoginChange, None, T32, Bound::MaxWait)),
        "amplihack.recipe.usage_limit_wait start reason=spend_limit wait=login_change \
         harness=claude reset=none until=2026-10-18 08:32:00 -07:00 bound=max_wait"
    );
}

/// The two tokens answer different questions and both are needed. `reason`
/// says which limit stopped the step, and is what a reader looking for a
/// spend limit greps for; `wait` says what the step is waiting for, and is
/// the only thing that distinguishes this wait from a reset wait on a
/// spend limit that did state a reset.
#[test]
fn the_reason_and_the_wait_token_are_not_the_same_field() {
    let l = stderr(&start(Shape::LoginChange, None, T32, Bound::MaxWait));
    assert!(l.contains(" reason=spend_limit "), "{l}");
    assert!(l.contains(" wait=login_change "), "{l}");
    for shape in [Shape::Reset, Shape::Capacity] {
        let l = stderr(&start(shape, None, T32, Bound::MaxWait));
        assert!(
            l.contains(" reason=usage_limit "),
            "{shape:?} is a session or weekly limit: {l}"
        );
    }
}

/// Issue #2911: a spend limit no longer closes a step, so there is no
/// closing line for one, and since issue #3037 there is no closing line for
/// any limit. The token a reader greps for is written by the
/// wait itself instead: on the `start` line's `reason=`, above, and on the
/// `waiting_for_capacity` line below.
#[test]
fn issue_2911_a_spend_limit_writes_its_token_on_the_wait_and_not_on_an_ending() {
    let l = stderr(&Event::WaitingForCapacity {
        harness: Harness::Claude,
        why: Waiting::SpendLimit,
        until: T32,
    });
    assert!(
        l.contains("waiting_for_capacity reason=spend_limit "),
        "{l}"
    );
}

// --- the recipe-log start line --------------------------------------------

#[test]
fn the_recipe_log_start_line_swaps_harness_for_account() {
    assert_eq!(
        recipe(
            &start(Shape::Reset, Some(T30), T32, Bound::ResetMargin),
            Some("work")
        )
        .unwrap(),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit account=work \
         reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 08:32:00 -07:00 bound=reset+margin"
    );
}

#[test]
fn the_recipe_log_keeps_the_wait_token_where_stderr_has_it() {
    assert_eq!(
        recipe(
            &start(Shape::LoginChange, None, T32, Bound::MaxWait),
            Some("work")
        )
        .unwrap(),
        "amplihack.recipe.usage_limit_wait start reason=spend_limit wait=login_change \
         account=work reset=none until=2026-10-18 08:32:00 -07:00 bound=max_wait"
    );
    assert_eq!(
        recipe(
            &start(Shape::Capacity, Some(T30), T32, Bound::ResetMargin),
            Some("c")
        )
        .unwrap(),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity \
         account=c reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 08:32:00 -07:00 \
         bound=reset+margin"
    );
}

#[test]
fn the_recipe_log_start_line_is_the_stderr_one_with_one_field_replaced() {
    for shape in [Shape::Reset, Shape::Capacity, Shape::LoginChange] {
        let e = start(shape, Some(T30), T32, Bound::MaxWait);
        let got = recipe(&e, Some("work")).unwrap();
        assert_eq!(
            got,
            stderr(&e).replace("harness=claude", "account=work"),
            "{shape:?}"
        );
    }
}

#[test]
fn the_reason_token_is_the_same_in_both_sinks() {
    // `reason=` must mean one thing across stderr and the recipe log, so a
    // reader greps once and finds the wait in whichever sink they have.
    for shape in [Shape::Reset, Shape::Capacity, Shape::LoginChange] {
        let e = start(shape, None, T32, Bound::MaxWait);
        let (l, s) = (recipe(&e, Some("w")).unwrap(), stderr(&e));
        let token = |line: &str| {
            line.split_whitespace()
                .find_map(|w| w.strip_prefix("reason="))
                .unwrap()
                .to_string()
        };
        assert_eq!(token(&l), token(&s), "{shape:?}: {l} / {s}");
    }
}

#[test]
fn a_spend_limits_wait_is_found_by_grepping_for_the_spend_limit() {
    // The field report: a reader looking for why a step stopped searched the
    // recipe log for `spend_limit` and found nothing, because the only line
    // that carried the token was the step's closing line on stderr.
    let l = recipe(
        &start(Shape::LoginChange, None, T32, Bound::MaxWait),
        Some("w"),
    )
    .unwrap();
    assert!(l.contains("start reason=spend_limit "), "{l}");
}

// --- the account name -----------------------------------------------------

#[test]
fn an_unreadable_login_name_is_unknown() {
    let l = recipe(
        &start(Shape::Reset, Some(T30), T32, Bound::ResetMargin),
        None,
    )
    .unwrap();
    assert!(l.contains(" account=unknown "), "{l}");
}

#[test]
fn a_name_outside_the_rotators_form_is_rejected_not_trimmed() {
    // Two logins must never collapse onto one identity in the log, so a
    // name that is not in the rotator's own form becomes `unknown` whole.
    let long = "a".repeat(65);
    for bad in [
        "",
        "has space",
        "has=equals",
        "has\nnewline",
        "me@example.invalid",
        "../escape",
        "/absolute/path",
        long.as_str(),
    ] {
        let l = recipe(&start(Shape::Reset, None, T32, Bound::MaxWait), Some(bad)).unwrap();
        assert!(
            l.contains(" account=unknown "),
            "{bad:?} must be rejected, not shortened: {l}"
        );
        assert!(!l.contains(bad) || bad.is_empty(), "{bad:?} leaked: {l}");
    }
}

#[test]
fn a_name_in_the_rotators_form_is_written_whole() {
    for good in ["a", "work", "second-2", "team_1.main", &"n".repeat(64)] {
        let l = recipe(&start(Shape::Reset, None, T32, Bound::MaxWait), Some(good)).unwrap();
        assert!(l.contains(&format!(" account={good} ")), "{good:?}: {l}");
    }
}

#[test]
fn one_line_always_and_nothing_but_printable_ascii() {
    let l = recipe(
        &start(Shape::Reset, Some(T30), T32, Bound::MaxWait),
        Some("w"),
    )
    .unwrap();
    assert_eq!(l.lines().count(), 1, "{l:?}");
    assert!(l.bytes().all(|b| (0x20..0x7f).contains(&b)), "{l:?}");
    assert!(l.starts_with(PREFIX), "{l:?}");
}

// --- the recipe-log end line ----------------------------------------------

#[test]
fn the_end_line_is_identical_in_both_sinks() {
    for end in [
        End::LoginChanged,
        End::ResetReached,
        End::MaxReached,
        End::RotatorCapacity,
        End::CapacityExpired,
        End::SessionEnded,
        End::OtherCapacity,
        End::Signal(libc::SIGTERM),
    ] {
        let e = Event::End(end);
        assert_eq!(recipe(&e, Some("work")).unwrap(), stderr(&e), "{end:?}");
        assert_eq!(
            recipe(&e, None).unwrap(),
            stderr(&e),
            "the end line carries no account at all: {end:?}"
        );
    }
}

// --- what never reaches the recipe log ------------------------------------

/// The recipe log carries a wait's `start`, `end`, `reentered` (#2857) and
/// `waiting_for_capacity` (#2911) lines and nothing else; the last two are
/// tested in their own sections below.
#[test]
fn progress_resume_and_skipped_lines_never_reach_the_recipe_log() {
    for e in [
        Event::Progress { remaining: 1740 },
        Event::Resume { attempt: 2 },
        Event::Skipped(Skip::Cannot(NoWait::NoLimitMessage)),
        Event::Skipped(Skip::Cannot(NoWait::PipedPrompt)),
    ] {
        assert_eq!(
            recipe(&e, Some("work")),
            None,
            "the recipe log carries a wait's start, end, reentered and \
             waiting_for_capacity lines only"
        );
    }
}

// --- Issue #2857: the re-entry line in its two sinks ---------------------------
//
// `reentered` is the third event that reaches the run's recipe log, beside a
// wait's `start` and `end`. It is the stderr line, unchanged, and it carries
// no `account=`: it says that this wait follows a contradicted signal, and
// which wait it is is already on the `start` line after it. Nothing in the
// control path reads the log back, so the line is for a person.

#[test]
fn issue_2857_the_re_entry_line_reaches_the_recipe_log_unchanged() {
    for met in [Met::UsageLimit, Met::RateLimit] {
        let e = Event::Reentered { met };
        assert_eq!(recipe(&e, Some("work")), Some(stderr(&e)), "{met:?}");
        assert_eq!(recipe(&e, None), Some(stderr(&e)), "{met:?}");
    }
}

#[test]
fn issue_2857_the_re_entry_line_never_carries_the_login() {
    let e = Event::Reentered {
        met: Met::RateLimit,
    };
    let l = recipe(&e, Some("work")).expect("it reaches the recipe log");
    assert!(!l.contains("account="), "{l}");
    assert!(!l.contains("work"), "{l}");
}

/// The recipe log still carries nothing but a wait's `start`, its `end` and
/// this line: no `progress`, `resume` or `skipped`.
#[test]
fn issue_2857_the_re_entry_line_is_the_only_addition_to_what_the_recipe_log_carries() {
    for e in [
        Event::Progress { remaining: 5 },
        Event::Resume { attempt: 2 },
        Event::Skipped(Skip::Cannot(NoWait::PipedPrompt)),
    ] {
        assert_eq!(recipe(&e, Some("work")), None);
    }
}

// --- Issue #2911: the waiting_for_capacity line in its two sinks ----------
//
// The fourth event that reaches the run's recipe log. It is the stderr line
// with `harness=` dropped and `account=` put in its place, exactly as the
// `start` line is, and the account is re-validated at the same point: a name
// outside the rotator's own form is written as `unknown` whole rather than
// trimmed to fit.

fn waiting(why: Waiting, until: i64) -> Event {
    Event::WaitingForCapacity {
        harness: Harness::Claude,
        why,
        until,
    }
}

#[test]
fn issue_2911_the_waiting_line_swaps_harness_for_account_as_the_start_line_does() {
    assert_eq!(
        recipe(&waiting(Waiting::ResetTooFar, T32), Some("work")).unwrap(),
        "amplihack.recipe.usage_limit_wait waiting_for_capacity reason=reset_too_far \
         account=work until=2026-10-18 08:32:00 -07:00"
    );
    assert_eq!(
        stderr(&waiting(Waiting::ResetTooFar, T32)),
        "amplihack.recipe.usage_limit_wait waiting_for_capacity reason=reset_too_far \
         harness=claude until=2026-10-18 08:32:00 -07:00"
    );
}

#[test]
fn issue_2911_a_waiting_line_with_no_usable_account_says_unknown() {
    for account in [
        None,
        Some(""),
        Some("a name with spaces"),
        Some("work\nstart reason=usage_limit account=other"),
        Some("../escape"),
    ] {
        let l = recipe(&waiting(Waiting::Credits, T32), account).unwrap();
        assert!(
            l.contains(" account=unknown "),
            "account {account:?} gave {l}"
        );
        assert!(!l.contains('\n'), "one line only: {l:?}");
    }
}

#[test]
fn issue_2911_every_reason_reaches_the_recipe_log() {
    for why in [
        Waiting::ResetTooFar,
        Waiting::NoReset,
        Waiting::SpendLimit,
        Waiting::Credits,
        Waiting::PassedReset,
    ] {
        let l = recipe(&waiting(why, T32), Some("work")).expect("it reaches the recipe log");
        assert!(
            l.starts_with(&format!(
                "{PREFIX}waiting_for_capacity reason={} ",
                why.as_str()
            )),
            "{l}"
        );
    }
}
