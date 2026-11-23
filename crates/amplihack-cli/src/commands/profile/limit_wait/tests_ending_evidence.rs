//! What the recipe runner makes of the one ending a step that has met a
//! limit can still reach: a prompt that could not be read to its end
//! (review of PR #2992, issue #3036).
//!
//! That ending is a lost prompt, not a limit ending: the step fails whether
//! or not it meets a limit, because its attempt was given less than its
//! task, and it is logged as `failed reason=prompt_cut_short`, never as a
//! skipped wait. It does end a step whose limit still stands, though, so it
//! is handed up as a *temporary* fault, and the run is resumed at the step,
//! whose whole prompt the runner sends again, instead of being recorded as
//! work that failed.
//!
//! That rests on data, not on the words the ending writes (review of PR
//! #2992, round 5). The supervisor reports the limit to the runner over the
//! recovery channel, the runner writes it into the run's result as
//! `failure_cause`, and the classifier reads that field. These tests build
//! that result and classify it with and without the sentence, so a reworded
//! sentence cannot change the verdict and a lost cause cannot hide behind
//! the sentence.
//!
//! The step-timeout ending this file also pinned is gone (issue #3037): a
//! step timeout no longer ends a limit wait, so there is no such ending to
//! classify. So is the piped-prompt ending for a copy that failed or could
//! not be set up: the copy is kept in memory, and a step whose copy cannot
//! be set up is not run (`limit_wait::scratch`).
//!
//! So the property is pinned here rather than described. The ending is
//! reduced to the lines the supervisor itself writes and classified exactly
//! as the runner would classify it.
//!
//! The lines themselves are built in `ending.rs`. Wired from
//! `limit_wait/mod.rs` with `#[cfg(test)] mod tests_ending_evidence;`.

use super::ending::prompt_lost_note;
use super::events::{Event, fmt_time, line};
use crate::commands::recipe::RecipeRunResult;
use crate::commands::recipe::run::failure_evidence::usage_limit_reset_hint;
use crate::commands::recipe::run::limit_patterns;
use crate::commands::recipe::run::{FailureClass, classify_failure_text, classify_run_failure};

/// 2026-10-18 08:30:00 -07:00, the reset the fixtures elsewhere use.
const RESET: i64 = 1_792_337_400;
/// Half an hour before it, as `TEST_NOW` is.
const NOW: i64 = RESET - 1800;

/// A cause of the shape `rerun::PROMPT_CUT_SHORT` gives: static, and never
/// built from what arrived on the pipe.
const CAUSE: &str = "its piped prompt could not be read to its end, so neither this attempt \
                     nor a later one can be given the whole of it";

/// Everything the prompt-loss ending writes, in the order it writes it.
fn prompt_lost_ending(reset: Option<i64>, now: i64) -> String {
    format!(
        "{}\n{}",
        prompt_lost_note(CAUSE, reset, now),
        line(&Event::PromptCutShort, &chrono::Local),
    )
}

fn endings(reset: Option<i64>, now: i64) -> [(&'static str, String); 1] {
    [("prompt cut short", prompt_lost_ending(reset, now))]
}

/// Review of PR #2992: the ending says which it is. It is a lost prompt,
/// and the limit is not why the step ends, so neither the sentence nor the
/// event reads as a wait refused to a limit.
#[test]
fn the_ending_names_a_lost_prompt_and_not_a_limit_ending() {
    for reset in [Some(RESET), Some(NOW - 1), None] {
        for (what, text) in endings(reset, NOW) {
            assert!(
                text.contains("failed because its prompt was lost, not because of its usage limit"),
                "{what}:\n{text}"
            );
            assert!(
                text.contains("fails whether or not it meets a limit"),
                "{what}:\n{text}"
            );
            assert!(
                text.lines()
                    .any(|l| l.ends_with("usage_limit_wait failed reason=prompt_cut_short")),
                "{what}:\n{text}"
            );
            assert!(!text.contains("skipped"), "{what}:\n{text}");
            assert!(!text.contains("piped_prompt"), "{what}:\n{text}");
        }
    }
}

/// The lines a prompt-cut-short step writes besides the ending's sentence:
/// the copier's report of the failed read, and `finish`'s refusal of the
/// attempt's success. Neither names the limit.
const LOSS_LINES: &str = "amplihack: this step's prompt could not be read to its end (Bad file \
     descriptor (os error 9)), so the attempt reading it is given only what arrived before \
     that: a prompt cut short, which it cannot tell from a whole one\n\
     amplihack: this step's prompt was cut short on its way to the attempt, so the attempt \
     was not given the whole task and its result is not this step's result";

/// The run result the recipe runner reports for a run whose one agent step
/// ended with `stderr`, and, when `recorded` is set, the structured cause
/// the runner writes for a step the supervisor reported as limited
/// (`failure_cause`, issue #2334). The shape is the runner's own JSON.
fn run_result(stderr: &str, recorded: bool) -> RecipeRunResult {
    let mut v = serde_json::json!({
        "recipe_name": "default-workflow",
        "success": false,
        "step_results": [{
            "step_id": "step-08-implement",
            "status": "failed",
            "error": "agent step exited with status 1",
            "recent_stderr": stderr.lines().collect::<Vec<_>>(),
        }],
    });
    if recorded {
        v["failure_cause"] = serde_json::json!({
            "class": "usage_limit",
            "origin": {"invocation_path": "default-workflow", "step_id": "step-08-implement"},
        });
    }
    serde_json::from_value(v).expect("the runner's result shape")
}

/// A step that ends this way is a temporary fault the run resumes from, not
/// work that failed, and that verdict comes from the cause the supervisor
/// recorded, not from the sentence (review of PR #2992, round 5).
///
/// The same run is classified with the whole ending and with the sentence
/// removed. Both are `usage_limit`: the sentence decides nothing.
#[test]
fn the_run_is_classed_a_usage_limit_from_the_recorded_cause_not_the_sentence() {
    for reset in [Some(RESET), Some(NOW - 1), None] {
        for (what, ending) in endings(reset, NOW) {
            let with_sentence = format!("{LOSS_LINES}\n{ending}");
            let without_sentence = format!(
                "{LOSS_LINES}\n{}",
                line(&Event::PromptCutShort, &chrono::Local)
            );
            for (which, stderr) in [
                ("with the sentence", &with_sentence),
                ("without it", &without_sentence),
            ] {
                let verdict = classify_run_failure(&run_result(stderr, true), "");
                assert_eq!(
                    verdict.class,
                    FailureClass::UsageLimit,
                    "{what} {which}, reset={reset:?}: {}",
                    verdict.reasoning()
                );
            }
        }
    }
}

/// The other half of the property above: with no recorded cause, the lines
/// a prompt-cut-short step writes about itself do not make the run a usage
/// limit. Without the sentence the run is `indeterminate`, so what makes it
/// resumable is the cause the supervisor recorded, which no rewording of a
/// log line can remove.
#[test]
fn without_the_recorded_cause_the_step_s_own_lines_do_not_class_a_usage_limit() {
    let lines = format!(
        "{LOSS_LINES}\n{}",
        line(&Event::PromptCutShort, &chrono::Local)
    );
    let verdict = classify_run_failure(&run_result(&lines, false), "");
    assert_eq!(
        verdict.class,
        FailureClass::Indeterminate,
        "{}",
        verdict.reasoning()
    );
    let recorded = classify_run_failure(&run_result(&lines, true), "");
    assert_eq!(recorded.class, FailureClass::UsageLimit);
    assert_eq!(
        recorded.signal,
        Some("durable scoped usage-limit evidence"),
        "{}",
        recorded.reasoning()
    );
}

/// The event line names the loss and not the limit, so on its own it reads
/// as no limit at all. That is right: the limit is not why the step ends,
/// and the run learns of it from the recorded cause.
#[test]
fn the_event_line_alone_is_not_a_usage_limit() {
    let failed = line(&Event::PromptCutShort, &chrono::Local);
    assert_eq!(
        classify_failure_text(&failed).0,
        FailureClass::Indeterminate,
        "{failed}"
    );
}

/// The recorded cause says that the step stopped on a limit but not when
/// the limit lifts. The ending says that, in the one form the runner's own
/// reader takes a reset from, so the verdict a person reads names the time.
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
