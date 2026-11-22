//! What the recipe runner makes of the two endings that can end a step
//! while its limit still stands (review of PR #2992, issues #3036, #3037).
//!
//! Neither ending is a limit being refused, and this PR's case for leaving
//! both in place is that a step which ends this way is handed up as a
//! *temporary* fault with the time it clears, so a supervising loop resumes
//! the run instead of recording work that failed. That case rests entirely
//! on the words these two endings write: the classifier reads the step's
//! evidence, and `skipped reason=piped_prompt` and `end reason=step timeout`
//! carry no limit marker of their own.
//!
//! So the property is pinned here rather than described. Each ending is
//! reduced to the lines the supervisor itself writes - which is all that is
//! left when a long wait's progress lines have pushed the harness's own
//! message out of the 40-line evidence tail - and classified exactly as the
//! runner would classify it.
//!
//! The lines themselves are built in `ending.rs`. Wired from
//! `limit_wait/mod.rs` with `#[cfg(test)] mod tests_ending_evidence;`.

use super::ending::{no_retry_note, step_timeout_note};
use super::events::{Event, FailReason, Skip, fmt_time, line};
use super::plan::NoWait;
use crate::commands::recipe::run::failure_evidence::usage_limit_reset_hint;
use crate::commands::recipe::run::limit_patterns;
use crate::commands::recipe::run::{FailureClass, classify_failure_text};

/// 2026-10-18 08:30:00 -07:00, the reset the fixtures elsewhere use.
const RESET: i64 = 1_792_337_400;
/// Half an hour before it, as `TEST_NOW` is.
const NOW: i64 = RESET - 1800;

/// A cause of the shape `rerun::why_no_copy` gives: static, and chosen from
/// the step's own arguments.
const CAUSE: &str = "its prompt arrives on an --input-format stream channel, which its caller \
                     holds open for the session, so there is no end of it to copy and no \
                     prompt to replay";

/// Everything a step-timeout ending writes, in the order it writes it.
fn step_timeout_ending(reset: Option<i64>, now: i64) -> String {
    format!(
        "{}\n{}\n{}",
        line(&Event::End(super::wait::End::StepTimeout), &chrono::Local),
        step_timeout_note(reset, now),
        line(&Event::Failed(FailReason::StepTimeout), &chrono::Local),
    )
}

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

fn endings(reset: Option<i64>, now: i64) -> [(&'static str, String); 2] {
    [
        ("step timeout", step_timeout_ending(reset, now)),
        ("piped prompt", piped_prompt_ending(reset, now)),
    ]
}

/// The whole of the PR's case for deferring #3036 and #3037: a step that
/// ends either way is a temporary fault the run resumes from, not work that
/// failed.
#[test]
fn both_endings_hand_the_runner_a_usage_limit() {
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

/// Why the sentence beside each event may not be reworded into one that
/// drops the words *usage limit*: the event line alone carries no marker,
/// so the sentence is the only thing keeping the ending out of
/// `indeterminate` - "we do not know" - which a supervising loop reads as
/// terminal.
#[test]
fn the_event_lines_alone_would_not_be_classified_as_a_usage_limit() {
    let skipped = line(
        &Event::Skipped(Skip::Cannot(NoWait::PipedPrompt)),
        &chrono::Local,
    );
    assert_eq!(
        classify_failure_text(&skipped).0,
        FailureClass::Indeterminate,
        "{skipped}"
    );
    let ended = line(&Event::End(super::wait::End::StepTimeout), &chrono::Local);
    assert_eq!(
        classify_failure_text(&ended).0,
        FailureClass::Indeterminate,
        "{ended}"
    );
}

/// Issue #3036 and #3037 both end with the same question for the layer
/// above: when is it worth starting this step again? Each ending answers it
/// in the one form the runner's own reader takes a reset from, so the
/// verdict a supervising loop reads names the time.
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

/// Every line of either ending is one printable ASCII line, as every other
/// line the wait writes is: the reset is formatted from an integer and the
/// cause is static text chosen from the step's arguments, so nothing the
/// harness printed reaches either sentence.
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
