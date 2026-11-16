//! Issue #2731: the wait's lines in their two sinks.
//!
//! The stderr line keeps its fields and order; a login-change wait's `start`
//! carries `wait=login_change` where a capacity wait carries `wait=capacity`.
//! The recipe-log `start` is that line with `harness=` dropped and
//! `account=` put in its place, and the recipe-log `end` is identical to the
//! stderr one, so one `grep` over both sinks reads as one stream.
//!
//! No other event reaches the recipe log: not `progress`, not `resume`, not
//! `skipped`, and not the closing `failed` line, whose `reason=spend_limit`
//! is a different token from the `start` line's `reason=usage_limit`.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_log_recipe;`.

use super::events::{Event, FailReason, Shape, Skip, line, recipe_line};
use super::plan::{Bound, NoWait};
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
        "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=login_change \
         harness=claude reset=none until=2026-10-18 08:32:00 -07:00 bound=max_wait"
    );
}

#[test]
fn the_closing_line_for_a_spend_limit_names_it() {
    assert_eq!(
        stderr(&Event::Failed(FailReason::SpendLimit)),
        "amplihack.recipe.usage_limit_wait failed reason=spend_limit: usage limit not lifted"
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
            Some("calypso")
        )
        .unwrap(),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=login_change \
         account=calypso reset=none until=2026-10-18 08:32:00 -07:00 bound=max_wait"
    );
    assert_eq!(
        recipe(
            &start(Shape::Capacity, Some(T30), T32, Bound::StepTimeout),
            Some("c")
        )
        .unwrap(),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity \
         account=c reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 08:32:00 -07:00 \
         bound=step_timeout"
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
fn the_reason_token_is_always_usage_limit_in_the_recipe_log() {
    // The closing line's `reason=spend_limit` is a different event and must
    // not split the meaning of `reason=` across the two sinks.
    let l = recipe(
        &start(Shape::LoginChange, None, T32, Bound::MaxWait),
        Some("w"),
    )
    .unwrap();
    assert!(l.contains("start reason=usage_limit "), "{l}");
    assert!(!l.contains("reason=spend_limit"), "{l}");
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
    for good in ["a", "work", "calypso-2", "team_1.main", &"n".repeat(64)] {
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
        End::StepTimeout,
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

#[test]
fn only_start_and_end_reach_the_recipe_log() {
    for e in [
        Event::Progress { remaining: 1740 },
        Event::Resume { attempt: 2 },
        Event::Skipped(Skip::AlreadyWaited),
        Event::Skipped(Skip::Cannot(NoWait::SpendLimit)),
        Event::Failed(FailReason::SpendLimit),
        Event::Failed(FailReason::AlreadyWaited),
        Event::Failed(FailReason::CapacityExpired),
        Event::Failed(FailReason::StepTimeout),
    ] {
        assert_eq!(
            recipe(&e, Some("work")),
            None,
            "only the start and end of a wait belong in the recipe log"
        );
    }
}
