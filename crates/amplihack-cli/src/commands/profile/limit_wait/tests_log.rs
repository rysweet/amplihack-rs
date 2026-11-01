//! The usage-limit wait's log lines (issue #1618): the documented fields
//! and order, times with a zone offset, and one printable ASCII line for any
//! input.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_log;`.

use super::events::{Event, FailReason, Skip, line};
use super::plan::{Bound, NoWait};
use super::wait::End;
use crate::commands::recipe::run::limit_patterns::{self, Harness};
use crate::commands::recipe::run::{FailureClass, classify_failure_text};

/// 2026-10-18 08:00:00 -07:00.
const T0: i64 = 1_792_335_600;
const PREFIX: &str = "amplihack.recipe.usage_limit_wait ";

const LA: chrono_tz::Tz = chrono_tz::America::Los_Angeles;

fn start(reset: i64, until: i64, bound: Bound) -> Event {
    Event::Start {
        harness: Harness::Claude,
        reset: Some(reset),
        until,
        bound,
        capacity: false,
    }
}

fn capacity_start(reset: Option<i64>, until: i64, bound: Bound) -> Event {
    Event::Start {
        harness: Harness::Claude,
        reset,
        until,
        bound,
        capacity: true,
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
         reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 08:32:00 -07:00 bound=reset+margin"
    );
}

#[test]
fn times_use_the_zone_offset_with_a_colon() {
    let l = line(
        &start(T0, T0, Bound::StepTimeout),
        &chrono_tz::Asia::Kolkata,
    );
    assert!(
        l.ends_with("until=2026-10-18 20:30:00 +05:30 bound=step_timeout"),
        "{l}"
    );
}

#[test]
fn every_other_event_has_its_documented_fields() {
    let cases = [
        (
            Event::Progress { remaining: 1740 },
            "progress remaining=1740s",
        ),
        (Event::End(End::ResetReached), "end reason=reset reached"),
        (Event::End(End::LoginChanged), "end reason=login changed"),
        (
            Event::End(End::RotatorCapacity),
            "end reason=rotator capacity",
        ),
        (Event::End(End::SessionEnded), "end reason=session ended"),
        (Event::End(End::MaxReached), "end reason=max reached"),
        (Event::End(End::StepTimeout), "end reason=step timeout"),
        (Event::End(End::Signal(libc::SIGTERM)), "end reason=signal"),
        (Event::Resume { attempt: 2 }, "resume attempt=2"),
        (Event::Skipped(Skip::Disabled), "skipped reason=disabled"),
        (
            Event::Skipped(Skip::AlreadyWaited),
            "skipped reason=already_waited",
        ),
        (
            Event::Skipped(Skip::NotSupervised),
            "skipped reason=not_supervised",
        ),
        (
            Event::Skipped(Skip::Cannot(NoWait::SpendLimit)),
            "skipped reason=spend_limit",
        ),
        // Rendering only: the upgrade to a capacity wait is decided before
        // logging and is tested in tests_plan.rs.
        (
            Event::Skipped(Skip::Cannot(NoWait::ResetTooFar)),
            "skipped reason=reset_too_far",
        ),
        (
            Event::Skipped(Skip::Cannot(NoWait::NoReset)),
            "skipped reason=no_reset",
        ),
        (
            Event::Skipped(Skip::Cannot(NoWait::NoLimitMessage)),
            "skipped reason=no_limit_message",
        ),
        (
            Event::Skipped(Skip::Cannot(NoWait::PipedPrompt)),
            "skipped reason=piped_prompt",
        ),
        (Event::Skipped(Skip::Ci), "skipped reason=ci"),
        (
            Event::End(End::CapacityExpired),
            "end reason=capacity expired",
        ),
        (Event::End(End::OtherCapacity), "end reason=login capacity"),
        (
            Event::Failed(FailReason::CapacityExpired),
            "failed reason=capacity_expired: usage limit not lifted",
        ),
        (
            Event::Failed(FailReason::StepTimeout),
            "failed reason=step_timeout: usage limit not lifted",
        ),
        (
            Event::Failed(FailReason::AlreadyWaited),
            "failed reason=already_waited: usage limit not lifted",
        ),
    ];
    for (event, rest) in cases {
        assert_eq!(line(&event, &LA), format!("{PREFIX}{rest}"));
    }
}

#[test]
fn every_line_is_one_printable_ascii_line_even_for_extreme_values() {
    let events = [
        start(i64::MAX, i64::MIN, Bound::ResetMargin),
        start(i64::MIN, i64::MAX, Bound::MaxWait),
        start(0, 0, Bound::StepTimeout),
        capacity_start(None, i64::MAX, Bound::MaxWait),
        capacity_start(Some(i64::MIN), i64::MIN, Bound::StepTimeout),
        Event::Progress {
            remaining: u64::MAX,
        },
        Event::Resume { attempt: u32::MAX },
        Event::End(End::Signal(i32::MIN)),
    ];
    for e in &events {
        let l = line(e, &LA);
        assert!(l.starts_with(PREFIX), "{l}");
        assert!(
            l.bytes().all(|b| (0x20..0x7f).contains(&b)),
            "non-printable byte in {l:?}"
        );
    }
}

// --- capacity waits (issue #1801) ----------------------------------------

#[test]
fn a_capacity_start_line_names_the_wait_after_the_reason() {
    assert_eq!(
        line(
            &capacity_start(Some(T0 + 1800), T0 + 21_600, Bound::MaxWait),
            &LA
        ),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity harness=claude \
         reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 14:00:00 -07:00 bound=max_wait"
    );
}

#[test]
fn a_capacity_start_line_with_no_reset_says_none() {
    assert_eq!(
        line(&capacity_start(None, T0, Bound::StepTimeout), &LA),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity harness=claude \
         reset=none until=2026-10-18 08:00:00 -07:00 bound=step_timeout"
    );
}

const FAILED: [FailReason; 3] = [
    FailReason::CapacityExpired,
    FailReason::StepTimeout,
    FailReason::AlreadyWaited,
];

#[test]
fn the_closing_line_is_classified_as_a_usage_limit() {
    for reason in FAILED {
        let l = line(&Event::Failed(reason), &LA);
        assert_eq!(classify_failure_text(&l).0, FailureClass::UsageLimit, "{l}");
    }
}

#[test]
fn the_closing_line_is_not_a_limit_message_so_it_starts_no_wait() {
    for reason in FAILED {
        let l = line(&Event::Failed(reason), &LA);
        assert_eq!(
            limit_patterns::limit_message(l.as_bytes(), b"", T0),
            None,
            "{l}"
        );
    }
}

/// AC4: a six-hour wait writes 360 progress lines, which push Claude's own
/// message out of the runner's 40-line evidence tail.
#[test]
fn the_class_survives_a_real_number_of_progress_lines() {
    let mut lines = vec![
        "You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)".to_string(),
        line(&capacity_start(None, T0 + 21_600, Bound::MaxWait), &LA),
    ];
    for k in 1..=360u64 {
        lines.push(line(
            &Event::Progress {
                remaining: 21_600 - k * 60,
            },
            &LA,
        ));
    }
    lines.push(line(&Event::End(End::CapacityExpired), &LA));
    lines.push(line(&Event::Failed(FailReason::CapacityExpired), &LA));
    let tail = lines[lines.len() - 40..].join("\n");
    assert_eq!(classify_failure_text(&tail).0, FailureClass::UsageLimit);
}

#[test]
fn no_event_can_hold_a_name_a_path_or_an_address() {
    let events = [
        capacity_start(None, T0, Bound::MaxWait),
        Event::End(End::OtherCapacity),
        Event::Failed(FailReason::CapacityExpired),
    ];
    for e in &events {
        let l = line(e, &LA);
        assert!(!l.contains('@') && !l.contains('/'), "{l}");
    }
}
