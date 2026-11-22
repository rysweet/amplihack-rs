//! Issue #2912: what counts as the model having replied during a run.
//!
//! The fixtures in `testdata/issue_2912/` are the lines a real Claude Code
//! 2.1.330 appended to a conversation's transcript when it was started with
//! `claude --continue`, with ids, paths and accounts replaced. `@NOW@`
//! stands for the time each line was written. How they were recorded is in
//! docs/howto/keep-sessions-running.md.

use super::progress::replied_since;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A start that was ended before any turn: a `bridge-session` and a
/// `cost-state` line, 841 bytes and no reply.
const START: &str = include_str!("testdata/issue_2912/start_without_a_turn.jsonl");
/// A turn whose request failed: the prompt, and the assistant line Claude
/// Code writes for the error with `"model":"<synthetic>"`.
const FAILED: &str = include_str!("testdata/issue_2912/failed_turn.jsonl");
/// A turn the model answered.
const REPLY: &str = include_str!("testdata/issue_2912/model_reply_turn.jsonl");

/// 2026-11-22T16:12:56Z, the launch of the run under test.
fn launch() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_795_363_976)
}

/// `lines` as written `offset_ms` after (or, negative, before) the launch.
fn at(lines: &str, offset_ms: i64) -> String {
    let t = chrono::DateTime::from_timestamp_millis(1_795_363_976_000 + offset_ms).unwrap();
    lines.replace("@NOW@", &t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

fn replied(transcript: &str) -> bool {
    replied_since(transcript.as_bytes(), launch())
}

#[test]
fn a_reply_the_model_wrote_during_the_run_is_work() {
    assert!(replied(&at(REPLY, 5_000)));
    assert!(
        replied(&(at(START, 0) + &at(REPLY, 3_600_000))),
        "hours later"
    );
}

/// What every start of a real Claude Code appends, with no turn behind it.
/// The transcript grows, so a measurement of its size would have read this
/// as work and hidden a session that crashes after starting.
#[test]
fn a_start_with_no_turn_grows_the_transcript_but_is_not_work() {
    assert_eq!(START.len(), 841);
    assert!(!replied(START));
    assert!(!replied(&START.repeat(5)), "five starts are no more work");
}

/// A failed request writes the prompt and an assistant line of Claude
/// Code's own. Neither is a reply, even though the line is marked as made
/// by the model and is dated during the run.
#[test]
fn a_failed_request_is_not_work() {
    let failed = at(FAILED, 5_000);
    assert!(failed.contains(r#""madeBy":"model""#), "the premise");
    assert!(!replied(&failed));
    // An error line that names a real model is still an error.
    let named = failed.replace(r#""model":"<synthetic>""#, r#""model":"claude-opus-5-5""#);
    assert_ne!(named, failed);
    assert!(!replied(&named));
}

/// `--continue --fork-session` wrote a new transcript holding the earlier
/// turns with the timestamps they were first written with. Those replies
/// were before the launch, so a copy of them is not work by this run.
#[test]
fn history_copied_from_before_the_launch_is_not_work() {
    let copied = at(REPLY, -60_000) + &at(FAILED, -30_000) + START;
    assert!(!replied(&copied));
    assert!(
        replied(&(copied + &at(REPLY, 1))),
        "a new turn after the copy"
    );
}

#[test]
fn the_launch_is_compared_to_the_millisecond() {
    assert!(
        replied(&at(REPLY, 0)),
        "a reply at the launch is this run's"
    );
    assert!(!replied(&at(REPLY, -1)), "one millisecond before is not");
}

/// Damage reads as no reply rather than ending the scan or panicking: a
/// line that is not JSON, a last line cut short, an assistant line with no
/// model or no usable timestamp. A reply after any of them still counts.
#[test]
fn damaged_lines_are_not_replies_and_do_not_hide_one() {
    let reply = at(REPLY, 5_000);
    let cut = &reply[..reply.find(r#""type":"assistant""#).unwrap() + 30];
    assert!(!replied(cut), "a reply cut short");
    let no_model = reply.replace(r#""model":"claude-opus-5-5","id""#, r#""id""#);
    let bad_time = reply.replace("2026-11-22T16:13:01.000Z", "yesterday");
    for (what, damaged) in [("no model", no_model), ("no usable time", bad_time)] {
        assert_ne!(damaged, reply, "{what}");
        assert!(!replied(&damaged), "{what}");
    }
    let damaged = format!("not json\n\u{0}\u{1}\n{{\"type\":\"assistant\"}}\n{cut}\n");
    assert!(!replied(&damaged));
    assert!(replied(&(damaged + &reply)));
}
