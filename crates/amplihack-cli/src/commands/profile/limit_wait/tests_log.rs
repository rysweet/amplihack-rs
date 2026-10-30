//! The usage-limit wait's log lines (issues #1618, #1761): the documented
//! fields and order, times with a zone offset, and one printable ASCII line
//! for any input. `start`, `resume` and every `skipped` name the step;
//! `end` and `resume` give the wait's own seconds. The plain lines and the
//! check on `login=` are in `tests_notes.rs`.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_log;`.

use super::events::{Event, Skip, line};
use super::plan::{Bound, NoWait};
use super::wait::End;
use crate::commands::recipe::run::limit_patterns::Harness;

/// 2026-10-18 08:00:00 -07:00.
const T0: i64 = 1_792_335_600;
const PREFIX: &str = "amplihack.recipe.usage_limit_wait ";
/// The step id of the documented examples.
const STEP: u32 = 48213;

const LA: chrono_tz::Tz = chrono_tz::America::Los_Angeles;

fn start(reset: i64, until: i64, bound: Bound) -> Event {
    Event::Start {
        harness: Harness::Claude,
        reset,
        until,
        bound,
        step: STEP,
        attempt: 1,
        rotator: true,
    }
}

#[test]
fn start_line_matches_the_documented_example() {
    assert_eq!(
        line(
            &start(1_792_337_400, 1_792_337_520, Bound::ResetMargin),
            &LA
        ),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit harness=claude \
         reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 08:32:00 -07:00 bound=reset+margin \
         step=48213 attempt=1 rotator=running"
    );
}

#[test]
fn start_names_the_attempt_that_hit_the_limit_and_a_missing_rotator() {
    let e = Event::Start {
        harness: Harness::Claude,
        reset: T0 + 1800,
        until: T0 + 600,
        bound: Bound::Budget,
        step: 7,
        attempt: 3,
        rotator: false,
    };
    assert!(
        line(&e, &LA).ends_with(
            " until=2026-10-18 08:10:00 -07:00 bound=budget step=7 attempt=3 rotator=none"
        ),
        "{}",
        line(&e, &LA)
    );
}

#[test]
fn times_use_the_zone_offset_with_a_colon() {
    let l = line(
        &start(T0, T0, Bound::StepTimeout),
        &chrono_tz::Asia::Kolkata,
    );
    assert!(
        l.contains(" until=2026-10-18 20:30:00 +05:30 bound=step_timeout step=48213 "),
        "{l}"
    );
}

#[test]
fn every_end_reason_is_followed_by_the_waits_own_seconds() {
    let cases = [
        (End::ResetReached, 1920, "reset reached waited=1920s"),
        (End::LoginChanged, 95, "login changed waited=95s"),
        (End::RotatorCapacity, 412, "rotator capacity waited=412s"),
        (End::SessionEnded, 3, "session ended waited=3s"),
        (End::MaxReached, 0, "max reached waited=0s"),
        (End::StepTimeout, 60, "step timeout waited=60s"),
        (End::Signal(libc::SIGTERM), 10, "signal waited=10s"),
        (End::BudgetSpent, 600, "budget spent waited=600s"),
    ];
    for (end, waited, rest) in cases {
        assert_eq!(
            line(&Event::End { end, waited }, &LA),
            format!("{PREFIX}end reason={rest}")
        );
    }
}

#[test]
fn resume_names_the_next_attempt_the_step_the_wait_and_the_login() {
    let resume = |login: Option<&str>| Event::Resume {
        attempt: 2,
        step: STEP,
        waited: 412,
        login: login.map(str::to_string),
    };
    assert_eq!(
        line(&resume(Some("team")), &LA),
        format!("{PREFIX}resume attempt=2 step=48213 waited=412s login=team")
    );
    assert_eq!(
        line(&resume(None), &LA),
        format!("{PREFIX}resume attempt=2 step=48213 waited=412s login=unknown")
    );
}

#[test]
fn progress_is_unchanged() {
    assert_eq!(
        line(&Event::Progress { remaining: 1740 }, &LA),
        format!("{PREFIX}progress remaining=1740s")
    );
}

#[test]
fn every_skip_names_the_step_and_already_waited_adds_no_rotator() {
    let cases = [
        (Skip::Disabled, "disabled step=48213"),
        (
            Skip::AlreadyWaited,
            "already_waited step=48213 rotator=none",
        ),
        (Skip::StepWaitsSpent, "step_waits_spent step=48213"),
        (Skip::BudgetSpent, "budget_spent step=48213"),
        (Skip::NotSupervised, "not_supervised step=48213"),
        (Skip::Ci, "ci step=48213"),
        (Skip::Cannot(NoWait::SpendLimit), "spend_limit step=48213"),
        (
            Skip::Cannot(NoWait::ResetTooFar),
            "reset_too_far step=48213",
        ),
        (Skip::Cannot(NoWait::NoReset), "no_reset step=48213"),
        (
            Skip::Cannot(NoWait::NoLimitMessage),
            "no_limit_message step=48213",
        ),
        (Skip::Cannot(NoWait::PipedPrompt), "piped_prompt step=48213"),
    ];
    for (skip, rest) in cases {
        assert_eq!(
            line(&Event::Skipped { skip, step: STEP }, &LA),
            format!("{PREFIX}skipped reason={rest}")
        );
    }
}

#[test]
fn every_line_is_one_printable_ascii_line_even_for_extreme_values() {
    let events = [
        start(i64::MAX, i64::MIN, Bound::ResetMargin),
        start(i64::MIN, i64::MAX, Bound::MaxWait),
        start(0, 0, Bound::StepTimeout),
        Event::Start {
            harness: Harness::Claude,
            reset: i64::MAX,
            until: i64::MAX,
            bound: Bound::Budget,
            step: u32::MAX,
            attempt: u32::MAX,
            rotator: false,
        },
        Event::Progress {
            remaining: u64::MAX,
        },
        Event::End {
            end: End::Signal(i32::MIN),
            waited: u64::MAX,
        },
        Event::Resume {
            attempt: u32::MAX,
            step: u32::MAX,
            waited: u64::MAX,
            login: Some("x\n\u{1b}[2Jy".to_string()),
        },
        Event::Skipped {
            skip: Skip::AlreadyWaited,
            step: u32::MAX,
        },
    ];
    for e in &events {
        let l = line(e, &LA);
        assert!(l.starts_with(PREFIX), "{l}");
        assert!(
            l.bytes().all(|b| (0x20..0x7f).contains(&b)),
            "non-printable byte in {l:?}"
        );
    }
    let extreme = line(
        &Event::Resume {
            attempt: u32::MAX,
            step: u32::MAX,
            waited: u64::MAX,
            login: None,
        },
        &LA,
    );
    assert_eq!(
        extreme,
        format!(
            "{PREFIX}resume attempt={} step={} waited={}s login=unknown",
            u32::MAX,
            u32::MAX,
            u64::MAX
        )
    );
}
