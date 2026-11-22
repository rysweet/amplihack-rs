//! The usage-limit wait's log lines (issue #1618): the documented fields
//! and order, times with a zone offset, and one printable ASCII line for any
//! input.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_log;`.

use super::Met;
use super::events::{Event, Shape, Skip, line};
use super::plan::{Bound, NoWait, Waiting};
use super::wait::End;
use crate::commands::recipe::run::limit_patterns::{self, Harness};

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
        shape: Shape::Reset,
    }
}

fn capacity_start(reset: Option<i64>, until: i64, bound: Bound) -> Event {
    Event::Start {
        harness: Harness::Claude,
        reset,
        until,
        bound,
        shape: Shape::Capacity,
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
    let l = line(&start(T0, T0, Bound::MaxWait), &chrono_tz::Asia::Kolkata);
    assert!(
        l.ends_with("until=2026-10-18 20:30:00 +05:30 bound=max_wait"),
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
        (Event::End(End::Signal(libc::SIGTERM)), "end reason=signal"),
        (Event::Resume { attempt: 2 }, "resume attempt=2"),
        (Event::Skipped(Skip::Disabled), "skipped reason=disabled"),
        (
            Event::Skipped(Skip::NotSupervised),
            "skipped reason=not_supervised",
        ),
        // Issue #2911: `spend_limit`, `reset_too_far` and `no_reset` are no
        // longer skips. They name why a *wait* has no reset to end on, on
        // the `waiting_for_capacity` line below, so the tokens an operator
        // greps for still appear and now mean "waiting" rather than "ended".
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
        start(0, 0, Bound::MaxWait),
        capacity_start(None, i64::MAX, Bound::MaxWait),
        capacity_start(Some(i64::MIN), i64::MIN, Bound::ResetMargin),
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
        line(&capacity_start(None, T0, Bound::MaxWait), &LA),
        "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity harness=claude \
         reset=none until=2026-10-18 08:00:00 -07:00 bound=max_wait"
    );
}

// --- waiting_for_capacity (issue #2911) ----------------------------------
//
// A wait that has no reset it can end on says so plainly, on stderr and in
// the run's recipe log, naming the harness or the login and why. Such a step
// may wait a long time, and an operator reading either sink must be able to
// see that it is waiting for capacity rather than hung. The reason comes
// first, because that is the question a reader of a waiting step asks.

fn waiting(why: Waiting, until: i64) -> Event {
    Event::WaitingForCapacity {
        harness: Harness::Claude,
        why,
        until,
    }
}

#[test]
fn issue_2911_the_waiting_line_names_the_reason_first_then_the_harness() {
    assert_eq!(
        line(&waiting(Waiting::ResetTooFar, T0 + 21_600), &LA),
        "amplihack.recipe.usage_limit_wait waiting_for_capacity reason=reset_too_far \
         harness=claude until=2026-10-18 14:00:00 -07:00"
    );
}

#[test]
fn issue_2911_each_reason_has_its_own_token() {
    let rows = [
        (Waiting::ResetTooFar, "reset_too_far"),
        (Waiting::NoReset, "no_reset"),
        (Waiting::SpendLimit, "spend_limit"),
        (Waiting::Credits, "credits"),
        (Waiting::PassedReset, "passed_reset"),
    ];
    for (why, token) in rows {
        let l = line(&waiting(why, T0), &LA);
        assert!(
            l.contains(&format!("waiting_for_capacity reason={token} ")),
            "{l}"
        );
    }
}

/// The reason set is closed and the line is built from fixed text, so no
/// harness output can reach either sink through it, however extreme the
/// instant.
#[test]
fn issue_2911_the_waiting_line_is_one_printable_ascii_line_for_any_instant() {
    for until in [i64::MIN, 0, T0, i64::MAX] {
        for why in [
            Waiting::ResetTooFar,
            Waiting::NoReset,
            Waiting::SpendLimit,
            Waiting::Credits,
            Waiting::PassedReset,
        ] {
            let l = line(&waiting(why, until), &LA);
            assert!(l.starts_with(PREFIX), "{l}");
            assert!(
                l.bytes().all(|b| (0x20..0x7f).contains(&b)),
                "non-printable byte in {l:?}"
            );
            assert!(!l.contains('/'), "{l}");
        }
    }
}

/// It is not itself a limit message, so a step reading its own log back
/// cannot start a wait from it.
#[test]
fn issue_2911_the_waiting_line_starts_no_wait_of_its_own() {
    let l = line(&waiting(Waiting::Credits, T0), &LA);
    assert_eq!(limit_patterns::limit_message(l.as_bytes(), b"", T0), None);
}

// Issue #2857: a re-entry on a usage limit that states a reset the clock has
// already passed is a capacity wait, and its `start` line prints that reset.
// The reset is whatever the message stated, so it can be any past instant: the
// line must still be one well-formed line.

#[test]
fn issue_2857_a_capacity_start_line_for_a_long_past_reset_is_well_formed() {
    let until = T0 + 3600;
    for (reset, shown) in [
        (0, "1969-12-31 16:00:00 -08:00"),
        (1, "1969-12-31 16:00:01 -08:00"),
        (T0 - 120, "2026-10-18 07:58:00 -07:00"),
    ] {
        assert_eq!(
            line(&capacity_start(Some(reset), until, Bound::MaxWait), &LA),
            format!(
                "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity \
                 harness=claude reset={shown} until=2026-10-18 09:00:00 -07:00 bound=max_wait"
            ),
            "reset {reset}"
        );
    }
}

#[test]
fn issue_2857_an_instant_the_zone_cannot_write_is_written_as_seconds() {
    let l = line(
        &capacity_start(Some(i64::MIN), T0 + 3600, Bound::MaxWait),
        &LA,
    );
    assert!(
        l.contains(" reset=@-9223372036854775808 until=2026-10-18 09:00:00 -07:00 "),
        "{l}"
    );
    assert!(l.is_ascii() && !l.contains('\n'), "{l:?}");
}

/// Issue #3037, review of PR #2992: no line the wait writes says the step
/// failed on a limit. `capacity_expired`, `spend_limit` and `already_waited`
/// went in #2911; the last closing line, `failed reason=step_timeout`, went
/// with the step-timeout row, because nothing enforces that variable on a
/// step and the wait was the only code that turned it into an ending. A
/// chain of waits now ends only when the limit lifts, on a stop signal or
/// with the step's session.
///
/// The match is exhaustive, so a new way for a wait to end does not compile
/// here until this test says what its line reads.
#[test]
fn issue_3037_no_end_of_a_wait_closes_the_step_as_failed() {
    fn every(e: End) -> End {
        match e {
            End::ResetReached
            | End::MaxReached
            | End::LoginChanged
            | End::RotatorCapacity
            | End::CapacityExpired
            | End::OtherCapacity
            | End::SessionEnded
            | End::Signal(_) => e,
        }
    }
    for end in [
        End::ResetReached,
        End::MaxReached,
        End::LoginChanged,
        End::RotatorCapacity,
        End::CapacityExpired,
        End::OtherCapacity,
        End::SessionEnded,
        End::Signal(libc::SIGTERM),
    ] {
        let l = line(&Event::End(every(end)), &LA);
        assert!(
            !l.contains("failed") && !l.contains("timeout"),
            "a wait's end is not a step's failure: {l}"
        );
    }
}

/// The same change seen from a wait that runs out: the step has not failed,
/// and the lines it is writing are not a failure to classify. A capacity wait reaching its deadline is followed by `resume`,
/// so nothing in that tail claims the step ended on a limit.
#[test]
fn issue_2911_a_wait_that_retries_writes_no_closing_line() {
    let lines = [
        line(&capacity_start(None, T0 + 21_600, Bound::MaxWait), &LA),
        line(&Event::Progress { remaining: 60 }, &LA),
        line(&Event::End(End::CapacityExpired), &LA),
        line(&Event::Resume { attempt: 2 }, &LA),
    ];
    let tail = lines.join("\n");
    assert!(
        !tail.contains("failed reason="),
        "a limit that is being waited out ends nothing: {tail}"
    );
}

#[test]
fn no_event_can_hold_a_name_a_path_or_an_address() {
    let events = [
        capacity_start(None, T0, Bound::MaxWait),
        Event::End(End::OtherCapacity),
        Event::WaitingForCapacity {
            harness: Harness::Claude,
            why: Waiting::ResetTooFar,
            until: T0 + 21_600,
        },
    ];
    for e in &events {
        let l = line(e, &LA);
        assert!(!l.contains('@') && !l.contains('/'), "{l}");
    }
}

// --- Issue #2857: the line a re-entry writes ------------------------------
//
// One line per re-entry, before the new wait's `start`: `reentered met=` and
// one of two fixed tokens naming the limit that met the retry. It is built
// from the closed `Met` enum, so no text from the attempt's output, no login
// name and no path can reach it. A reader grepping the log can then tell a
// second `start` that follows a contradicted signal from one that follows a
// new login.

#[test]
fn issue_2857_a_re_entry_line_names_the_limit_that_met_the_retry() {
    assert_eq!(
        line(
            &Event::Reentered {
                met: Met::UsageLimit
            },
            &LA
        ),
        "amplihack.recipe.usage_limit_wait reentered met=usage_limit"
    );
    assert_eq!(
        line(
            &Event::Reentered {
                met: Met::RateLimit
            },
            &LA
        ),
        "amplihack.recipe.usage_limit_wait reentered met=rate_limit"
    );
}

#[test]
fn issue_2857_a_re_entry_line_has_no_other_field_and_no_zone() {
    for met in [Met::UsageLimit, Met::RateLimit] {
        let l = line(&Event::Reentered { met }, &LA);
        let rest = l.strip_prefix(PREFIX).expect("the prefix");
        let words: Vec<&str> = rest.split(' ').collect();
        assert_eq!(words.len(), 2, "{l}");
        assert_eq!(words[0], "reentered");
        assert!(words[1].starts_with("met="), "{l}");
        assert!(l.is_ascii() && !l.contains('\n'), "{l:?}");
    }
}

// An attempt's `Output` can hold 64 KiB of the agent's own text. It is matched
// for the two limit readings and dropped, and the type is built so that nothing
// later can print it by accident: no `Debug` for a stray `{:?}`, no `Display`,
// no `Clone` to keep a copy past the match, no `Serialize` to put it in a
// record. A derive added to it fails here, with the name of the trait.
//
// Each probe resolves, on stable Rust, to the inherent constant when the type
// has the trait and to the fallback trait's constant when it does not.
macro_rules! implements {
    ($t:ty: $tr:path) => {{
        struct Probe<T: ?Sized>(std::marker::PhantomData<T>);
        #[allow(dead_code)]
        impl<T: ?Sized + $tr> Probe<T> {
            const YES: bool = true;
        }
        #[allow(dead_code)]
        trait Otherwise {
            const YES: bool = false;
        }
        impl<T: ?Sized> Otherwise for Probe<T> {}
        <Probe<$t>>::YES
    }};
}

#[test]
fn issue_2857_the_output_a_limit_is_read_from_cannot_be_printed_copied_or_recorded() {
    use crate::commands::profile::attempt::Output;
    assert!(
        implements!(String: std::fmt::Debug),
        "the probe finds a trait a type has"
    );
    assert!(
        !implements!(Output: std::fmt::Debug),
        "Output must not be Debug"
    );
    assert!(
        !implements!(Output: std::fmt::Display),
        "Output must not be Display"
    );
    assert!(!implements!(Output: Clone), "Output must not be Clone");
    assert!(
        !implements!(Output: serde::Serialize),
        "Output must not be Serialize"
    );
}
