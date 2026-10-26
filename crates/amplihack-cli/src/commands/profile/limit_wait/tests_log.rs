//! The usage-limit wait's log lines (issue #1618): the documented fields
//! and order, times with a zone offset, and one printable ASCII line for any
//! input.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_log;`.

use super::plan::{Bound, NoWait};
use super::wait::End;
use super::{Event, Skip, line};
use crate::commands::recipe::run::limit_patterns::Harness;

/// 2026-10-18 08:00:00 -07:00.
const T0: i64 = 1_792_335_600;
const PREFIX: &str = "amplihack.recipe.usage_limit_wait ";

const LA: chrono_tz::Tz = chrono_tz::America::Los_Angeles;

fn start(reset: i64, until: i64, bound: Bound) -> Event {
    Event::Start {
        harness: Harness::Claude,
        reset,
        until,
        bound,
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
