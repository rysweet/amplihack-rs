//! What the recipe runner makes of the one ending a step that has met a
//! limit can still reach: a prompt that could not be read to its end
//! (review of PR #2992, issue #3036).
//!
//! That ending is not the limit's - the step fails whether or not it meets
//! one, because its attempt was given less than its task - but it does end
//! a step whose limit still stands, so it is handed up as a *temporary*
//! fault with the time the limit clears, and a supervising loop resumes the
//! run instead of recording work that failed. That rests entirely on the
//! words the ending writes: the classifier reads the step's evidence, and
//! `skipped reason=piped_prompt` carries no limit marker of its own.
//!
//! The step-timeout ending this file also pinned is gone (issue #3037): a
//! step timeout no longer ends a limit wait, so there is no such ending to
//! classify.
//!
//! So the property is pinned here rather than described. The ending is
//! reduced to the lines the supervisor itself writes and classified exactly
//! as the runner would classify it.
//!
//! The lines themselves are built in `ending.rs`. Wired from
//! `limit_wait/mod.rs` with `#[cfg(test)] mod tests_ending_evidence;`.

use super::ending::no_retry_note;
use super::events::{Event, Skip, fmt_time, line};
use super::plan::NoWait;
use crate::commands::recipe::run::failure_evidence::usage_limit_reset_hint;
use crate::commands::recipe::run::limit_patterns;
use crate::commands::recipe::run::{FailureClass, classify_failure_text};

/// 2026-10-18 08:30:00 -07:00, the reset the fixtures elsewhere use.
const RESET: i64 = 1_792_337_400;
/// Half an hour before it, as `TEST_NOW` is.
const NOW: i64 = RESET - 1800;

/// A cause of the shape `rerun::PROMPT_CUT_SHORT` gives: static, and never
/// built from what arrived on the pipe.
const CAUSE: &str = "its piped prompt could not be read to its end, so neither this attempt \
                     nor a later one can be given the whole of it";

/// Everything a piped-prompt ending writes, in the order it writes it.
fn piped_prompt_ending(reset: Option<i64>, now: i64) -> String {
    format!(
        "{}\n{}",
        no_retry_note(CAUSE, reset, now),
        line(
            &Event::Skipped(Skip::Cannot(NoWait::PipedPrompt)),
            &chrono::Local
        ),
    )
}

fn endings(reset: Option<i64>, now: i64) -> [(&'static str, String); 1] {
    [("piped prompt", piped_prompt_ending(reset, now))]
}

/// A step that ends this way is a temporary fault the run resumes from, not
/// work that failed.
#[test]
fn the_ending_hands_the_runner_a_usage_limit() {
    for reset in [Some(RESET), Some(NOW - 1), None] {
        for (what, text) in endings(reset, NOW) {
            assert_eq!(
                classify_failure_text(&text).0,
                FailureClass::UsageLimit,
                "{what} with reset={reset:?}:\n{text}"
            );
        }
    }
}

/// Why the sentence beside the event may not be reworded into one that
/// drops the words *usage limit*: the event line alone carries no marker,
/// so the sentence is the only thing keeping the ending out of
/// `indeterminate` - "we do not know" - which a supervising loop reads as
/// terminal.
#[test]
fn the_event_line_alone_would_not_be_classified_as_a_usage_limit() {
    let skipped = line(
        &Event::Skipped(Skip::Cannot(NoWait::PipedPrompt)),
        &chrono::Local,
    );
    assert_eq!(
        classify_failure_text(&skipped).0,
        FailureClass::Indeterminate,
        "{skipped}"
    );
}

/// The ending leaves the layer above one question: when is it worth
/// starting this step again? It answers in the one form the runner's own
/// reader takes a reset from, so the verdict a supervising loop reads names
/// the time.
#[test]
fn each_ending_states_the_reset_where_the_runner_reads_one() {
    let want = fmt_time(RESET, &chrono::Local);
    for (what, text) in endings(Some(RESET), NOW) {
        assert_eq!(
            usage_limit_reset_hint(&text).as_deref(),
            Some(want.as_str()),
            "{what}:\n{text}"
        );
    }
}

/// A limit that stated no reset says so, rather than naming a time nothing
/// measured. The hint is then absent and the verdict keeps its own wording.
#[test]
fn an_ending_with_no_reset_states_none() {
    for (what, text) in endings(None, NOW) {
        assert_eq!(usage_limit_reset_hint(&text), None, "{what}:\n{text}");
        assert!(
            text.contains("no reset is known for this limit"),
            "{what} says so in words:\n{text}"
        );
    }
}

/// A reset already behind the clock is stated as past, never as a time to
/// come back at: `Waiting::PassedReset` waits on a reading rather than on a
/// time that has been and gone (issue #2857), and a wait that runs to its
/// own reset ends past it too, which is how the held-open capture case
/// reaches this. The step says the reset has passed, so nothing should be
/// scheduled on it, and does not write it where the runner's reset reader
/// would quote it back as a time to wait for. It does not claim the limit
/// has lifted: the step never re-ran, so it does not know.
#[test]
fn a_reset_already_behind_the_clock_is_stated_as_past() {
    for r in [NOW - 1, NOW] {
        for (what, text) in endings(Some(r), NOW) {
            assert_eq!(usage_limit_reset_hint(&text), None, "{what}:\n{text}");
            assert!(
                text.contains(
                    "has already passed, so it does not say when to start this step again"
                ),
                "{what} says the reset has passed:\n{text}"
            );
            assert!(
                text.contains(&fmt_time(r, &chrono::Local)),
                "{what} still names it:\n{text}"
            );
        }
    }
    // One second later it is a time to come back at, and is written as one.
    for (what, text) in endings(Some(NOW + 1), NOW) {
        assert_eq!(
            usage_limit_reset_hint(&text).as_deref(),
            Some(fmt_time(NOW + 1, &chrono::Local).as_str()),
            "{what}:\n{text}"
        );
    }
}

/// The supervisor's own sentences are read by a parent amplihack when this
/// step is itself a supervised attempt, so none of them may have the shape
/// of a harness's limit message: a step must never start a wait on words
/// the wait itself wrote.
#[test]
fn no_ending_of_ours_reads_back_as_a_harness_limit_message() {
    for reset in [Some(RESET), Some(NOW - 1), None] {
        for (what, text) in endings(reset, NOW) {
            assert_eq!(
                limit_patterns::limit_message(text.as_bytes(), b"", RESET),
                None,
                "{what} on stderr:\n{text}"
            );
            assert_eq!(
                limit_patterns::limit_message(b"", text.as_bytes(), RESET),
                None,
                "{what} on stdout:\n{text}"
            );
        }
    }
}

/// Every line of the ending is one printable ASCII line, as every other
/// line the wait writes is: the reset is formatted from an integer and the
/// cause is static text chosen from the step's arguments, so nothing the
/// harness printed reaches the sentence.
#[test]
fn every_line_of_an_ending_is_one_printable_ascii_line() {
    for reset in [Some(RESET), Some(i64::MIN), Some(i64::MAX), None] {
        for (what, text) in endings(reset, NOW) {
            for l in text.lines() {
                assert!(
                    l.is_ascii() && !l.chars().any(|c| c.is_ascii_control()),
                    "{what}: {l:?}"
                );
                assert!(!l.trim().is_empty(), "{what}: blank line in\n{text}");
            }
        }
    }
}
