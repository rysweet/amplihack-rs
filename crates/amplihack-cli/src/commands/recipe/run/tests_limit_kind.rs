//! Issue #2911: Codex credit exhaustion is a usage limit, and every limit
//! message carries the kind of limit it states.
//!
//! Contract (these tests fail until it exists):
//! - `limit_patterns::LimitKind { Session, Weekly, Spend, Credits }`.
//! - `LimitMessage { harness, kind, reset }` replaces the `spend` field;
//!   `LimitMessage::spend()` is `kind == LimitKind::Spend`.
//! - `Harness::Codex` carries `"your workspace is out of credits"` and
//!   classifies as `usage_limit`; `"credit balance is too low"` stays
//!   `environmental`.
//! - A second harness row changes nothing for `epoch_marker` or `rate_limit`.
//! - Unrecognised wording reads as `Session`, the shortest window.
//!
//! Wired from `recipe/run/mod.rs` with `#[cfg(test)] mod tests_limit_kind;`.

use super::limit_patterns::{
    Harness, LimitKind, all_markers, epoch_marker, limit_message, rate_limit,
};
use super::tests_limit_patterns::{SESSION, SPEND, SPEND_SESSION, SPEND_WEEKLY, WEEKLY};
use super::{FailureClass, classify_failure_text};

const NOW: i64 = 1_792_335_600;
const CODEX: &str = "Your workspace is out of credits. Add credits to continue.";

fn kind_of(stderr: &str) -> Option<LimitKind> {
    limit_message(stderr.as_bytes(), b"", NOW).map(|m| m.kind)
}

#[test]
fn the_codex_phrase_classifies_as_a_usage_limit() {
    assert_eq!(classify_failure_text(CODEX).0, FailureClass::UsageLimit);
    assert!(all_markers().contains(&"your workspace is out of credits"));
}

#[test]
fn the_anthropic_credit_message_stays_environmental() {
    assert_eq!(
        classify_failure_text("Credit balance is too low").0,
        FailureClass::Environmental
    );
}

#[test]
fn the_codex_phrase_is_read_as_a_credits_limit_with_no_reset() {
    let m = limit_message(CODEX.as_bytes(), b"", NOW).expect("a limit message");
    assert_eq!(m.harness, Harness::Codex);
    assert_eq!(m.kind, LimitKind::Credits);
    assert_eq!(m.reset, None);
    assert!(!m.spend());
}

#[test]
fn the_codex_phrase_inside_a_sentence_is_not_a_limit_message() {
    for text in [
        "I read that your workspace is out of credits, so I stopped",
        "the error text was: Your workspace is out of credits. Add credits",
    ] {
        let line = format!("{text}\nmore output after it");
        assert_eq!(limit_message(b"", line.as_bytes(), NOW), None, "{text}");
    }
}

#[test]
fn the_codex_phrase_as_the_last_stdout_line_counts() {
    let out = format!("working\n{CODEX}\n");
    let m = limit_message(b"", out.as_bytes(), NOW).expect("a limit message");
    assert_eq!(m.kind, LimitKind::Credits);
}

#[test]
fn a_second_harness_row_does_not_turn_every_line_into_an_epoch_marker() {
    // An empty marker would match any line and return Some(None) here.
    assert_eq!(
        epoch_marker(b"ordinary stderr\n", b"ordinary stdout\n"),
        None
    );
    assert_eq!(epoch_marker(CODEX.as_bytes(), b""), None);
    assert_eq!(
        epoch_marker(b"Claude AI usage limit reached|1760000000\n", b""),
        Some(Some(1_760_000_000))
    );
}

#[test]
fn a_second_harness_row_leaves_the_rate_limit_reading_alone() {
    assert!(!rate_limit(CODEX.as_bytes(), b""));
    assert!(rate_limit(b"API Error: 429 too many requests\n", b""));
}

#[test]
fn claude_is_read_before_codex() {
    assert_eq!(Harness::ALL.first(), Some(&Harness::Claude));
    assert!(Harness::ALL.contains(&Harness::Codex));
    let both = format!("{SESSION}\n{CODEX}\n");
    let m = limit_message(both.as_bytes(), b"", NOW).unwrap();
    assert_eq!(m.harness, Harness::Claude);
}

#[test]
fn the_kind_is_read_from_the_messages_own_words() {
    assert_eq!(kind_of(SESSION), Some(LimitKind::Session));
    assert_eq!(kind_of(WEEKLY), Some(LimitKind::Weekly));
    assert_eq!(kind_of(SPEND), Some(LimitKind::Spend));
    assert_eq!(
        kind_of("You've hit your 5-hour limit · resets 1pm (America/Los_Angeles)"),
        Some(LimitKind::Session)
    );
}

#[test]
fn a_spend_message_that_also_states_a_window_is_still_a_spend_limit() {
    for text in [SPEND_SESSION, SPEND_WEEKLY] {
        let m = limit_message(text.as_bytes(), b"", NOW).unwrap();
        assert_eq!(m.kind, LimitKind::Spend);
        assert!(m.spend());
        assert!(m.reset.is_some(), "the stated window reset is kept");
    }
}

#[test]
fn unrecognised_wording_reads_as_the_shortest_window() {
    let m = limit_message(
        "You've hit your opus limit · resets 1pm (America/Los_Angeles)".as_bytes(),
        b"",
        NOW,
    )
    .unwrap();
    assert_eq!(m.kind, LimitKind::Session);
}
