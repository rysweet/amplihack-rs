//! Issue #2952: the limit lines Codex printed on 27 November 2026 are read as
//! limits, and a model's sentence about them is not.
//!
//! The four fixture lines are the captured text, unchanged. Each is read by
//! `limit_message` as Codex's own message, as the last stderr line and as the
//! last non-empty stdout line, and classed `usage_limit` by
//! `classify_failure_text`, alone and inside the runner's failure wrapper.

use super::limit_patterns::{Harness, LimitKind, limit_message, rate_limit};
use super::{FailureClass, classify_failure_text};

const NOW: i64 = 1_792_335_600;

pub(super) const CREDITS_ERROR: &str =
    "ERROR: Your workspace is out of credits. Add credits to continue.";
pub(super) const CREDITS_REFILL: &str =
    "Your workspace is out of credits. Ask your workspace owner to refill in order to continue.";
pub(super) const SERVICE_RATE: &str = "agent step failed: Codex exec failed with status exit status: 1: service rate limit; retry later or check account quota";
pub(super) const USAGE: &str = "Usage limit reached. You've reached your usage limit. Increase your limits to continue using codex.";

const FIXTURES: [(&str, LimitKind); 4] = [
    (CREDITS_ERROR, LimitKind::Credits),
    (CREDITS_REFILL, LimitKind::Credits),
    (SERVICE_RATE, LimitKind::Session),
    (USAGE, LimitKind::Session),
];

#[test]
fn each_fixture_line_on_stderr_is_codexs_own_limit_message() {
    for (line, kind) in FIXTURES {
        let stderr = format!("working on the step\n{line}\n");
        let m = limit_message(stderr.as_bytes(), b"attempt 1 output\n", NOW)
            .unwrap_or_else(|| panic!("not read as a limit: {line:?}"));
        assert_eq!(m.harness, Harness::Codex, "{line:?}");
        assert_eq!(m.kind, kind, "{line:?}");
        assert_eq!(m.reset, None, "{line:?}");
        assert!(!m.spend(), "{line:?}");
    }
}

#[test]
fn each_fixture_line_as_the_last_stdout_line_is_codexs_own_limit_message() {
    for (line, kind) in FIXTURES {
        let stdout = format!("attempt 1 output\n{line}\n\n");
        let m = limit_message(b"", stdout.as_bytes(), NOW)
            .unwrap_or_else(|| panic!("not read as a limit: {line:?}"));
        assert_eq!((m.harness, m.kind), (Harness::Codex, kind), "{line:?}");
    }
}

#[test]
fn each_fixture_line_classes_as_a_usage_limit_alone_and_wrapped() {
    for (line, _) in FIXTURES {
        assert_eq!(
            classify_failure_text(line).0,
            FailureClass::UsageLimit,
            "{line:?}"
        );
        let wrapped = format!(
            "Step 'step-04-address-blockers' failed: agent step failed: amplihack codex \
             failed (exit 1)\n--- stderr (tail) ---\n{line}\n"
        );
        assert_eq!(
            classify_failure_text(&wrapped).0,
            FailureClass::UsageLimit,
            "{wrapped:?}"
        );
    }
    // The runner's own wording for the rate line, with the step named in
    // front, as #2952's second comment quotes it.
    let runner = format!("Step 'decide' failed: {SERVICE_RATE}");
    assert_eq!(classify_failure_text(&runner).0, FailureClass::UsageLimit);
}

#[test]
fn no_fixture_line_is_read_as_a_429_envelope() {
    for (line, _) in FIXTURES {
        assert!(!rate_limit(line.as_bytes(), b""), "{line:?}");
    }
}

#[test]
fn a_models_sentence_about_the_phrases_is_not_a_limit_message() {
    for text in [
        "I saw ERROR: Your workspace is out of credits. Add credits to continue.",
        "The log said your workspace is out of credits, so I stopped.",
        "Codex reported: Usage limit reached. You've reached your usage limit.",
        "I checked the usage limit reached. You've reached your usage limit text.",
        "The step printed service rate limit; retry later or check account quota",
        "Codex exec failed with status 1 and then a service rate limit; retry later or check account quota was mentioned",
        "error: Your workspace is out of credits. Add credits to continue.",
        "ERROR:  Your workspace is out of credits.",
        "ERROR: ERROR: Your workspace is out of credits.",
    ] {
        assert_eq!(
            limit_message(text.as_bytes(), b"", NOW),
            None,
            "stderr {text:?}"
        );
        let stdout = format!("{text}\n");
        assert_eq!(
            limit_message(b"", stdout.as_bytes(), NOW),
            None,
            "stdout {text:?}"
        );
    }
}

#[test]
fn a_fixture_line_followed_by_more_model_text_on_stdout_is_not_a_limit_message() {
    for (line, _) in FIXTURES {
        let stdout = format!("{line}\nThat is what the tool printed earlier.\n");
        assert_eq!(limit_message(b"", stdout.as_bytes(), NOW), None, "{line:?}");
    }
}
