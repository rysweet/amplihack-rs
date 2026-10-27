//! `limit_patterns` (issue #1618): the markers the classifier reads, the
//! epoch reader the auto-switch ledger uses, the shaped limit-message reader
//! the usage-limit wait uses (with messages seen on real hosts), and the
//! operator-facing reset hint.
//!
//! Wired from `recipe/run/mod.rs` with `#[cfg(test)] mod tests_limit_patterns;`.

use super::limit_patterns::{Harness, LimitMessage, epoch_marker, limit_message, reset_hint};
use super::{FailureClass, classify_failure_text};

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;
/// 2026-10-18 08:30:00 -07:00.
const LA_0830: i64 = 1_792_337_400;
/// 2026-10-18 13:00:00 -07:00.
const LA_1300: i64 = 1_792_353_600;

/// Messages seen on real hosts, most frequent first.
pub(super) const SPEND: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage";
pub(super) const SPEND_SESSION: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your session limit resets 1pm (America/Los_Angeles)";
pub(super) const SPEND_WEEKLY: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Oct 8, 12am (America/Los_Angeles)";
pub(super) const WEEKLY: &str =
    "You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)";
pub(super) const SESSION: &str = "You've hit your session limit · resets 1pm (America/Los_Angeles)";

fn epoch_line(rest: &str) -> String {
    format!("Claude AI usage limit {}|{rest}", "reached")
}

fn epoch(stderr: &[u8], stdout: &[u8]) -> Option<i64> {
    epoch_marker(stderr, stdout).flatten()
}

fn msg(reset: Option<i64>, spend: bool) -> Option<LimitMessage> {
    Some(LimitMessage {
        harness: Harness::Claude,
        spend,
        reset,
    })
}

#[test]
fn every_marker_still_classifies_as_a_usage_limit() {
    for m in super::limit_patterns::all_markers() {
        let text = format!("Notice: {m} reached. Try again later.");
        assert_eq!(
            classify_failure_text(&text).0,
            FailureClass::UsageLimit,
            "marker {m:?}"
        );
    }
    // Out of credit is environmental, not a limit a wait could outlast.
    assert_ne!(
        classify_failure_text("Credit balance is too low").0,
        FailureClass::UsageLimit
    );
}

#[test]
fn epoch_reset_reads_the_last_marker_line_of_stderr() {
    let stderr = format!(
        "{}\nworking\n{}\n",
        epoch_line("1792336000"),
        epoch_line("1792337400")
    );
    assert_eq!(epoch(stderr.as_bytes(), b""), Some(LA_0830));
}

#[test]
fn epoch_reset_falls_back_to_stdout() {
    let stdout = format!("partial\n{}\n", epoch_line("1792337400"));
    assert_eq!(epoch(b"no marker here\n", stdout.as_bytes()), Some(LA_0830));
}

#[test]
fn epoch_reset_is_not_clamped() {
    let far = (NOW + 100 * 86_400).to_string();
    assert_eq!(
        epoch(epoch_line(&far).as_bytes(), b""),
        Some(NOW + 100 * 86_400)
    );
    assert_eq!(epoch(epoch_line("1").as_bytes(), b""), Some(1));
}

#[test]
fn epoch_reset_accepts_one_to_nineteen_digits_only() {
    assert_eq!(epoch(epoch_line("7").as_bytes(), b""), Some(7));
    assert_eq!(
        epoch(epoch_line("9223372036854775807").as_bytes(), b""),
        Some(i64::MAX),
        "19 digits that fit"
    );
    for bad in [
        "",
        "+1792337400",
        "-1792337400",
        " 1792337400",
        "9223372036854775808",  // 19 digits, overflows
        "9999999999999999999",  // 19 digits, overflows
        "17923374000000000000", // 20 digits
        "00000000000000000001", // 20 digits, small value
        "abc",
        "١٢٣", // non-ASCII digits
    ] {
        assert_eq!(
            epoch(epoch_line(bad).as_bytes(), b""),
            None,
            "suffix {bad:?}"
        );
    }
}

#[test]
fn epoch_reset_ignores_trailing_text_after_the_digits() {
    assert_eq!(
        epoch(epoch_line("1792337400 (local)").as_bytes(), b""),
        Some(LA_0830)
    );
}

#[test]
fn the_real_messages_are_read_from_stderr() {
    assert_eq!(
        limit_message(SESSION.as_bytes(), b"", NOW),
        msg(Some(LA_1300), false)
    );
    assert_eq!(limit_message(SPEND.as_bytes(), b"", NOW), msg(None, true));
    assert_eq!(
        limit_message(SPEND_SESSION.as_bytes(), b"", NOW),
        msg(Some(LA_1300), true)
    );
    // Oct 8 has passed on 2026-10-18, so next year's; the plan refuses it.
    let weekly = limit_message(WEEKLY.as_bytes(), b"", NOW).unwrap();
    assert!(!weekly.spend);
    assert!(weekly.reset.is_some_and(|r| r > NOW + 300 * 86_400));
}

#[test]
fn a_spend_limit_line_carries_the_session_or_weekly_reset_it_states() {
    let weekly = limit_message(SPEND_WEEKLY.as_bytes(), b"", NOW).unwrap();
    assert!(weekly.spend);
    assert!(
        weekly.reset.is_some_and(|r| r > NOW + 300 * 86_400),
        "next Oct 8"
    );
}

#[test]
fn the_last_line_of_stdout_counts_only_when_it_is_the_message() {
    let out = format!("attempt output\n{SESSION}\n\n");
    assert_eq!(
        limit_message(b"", out.as_bytes(), NOW),
        msg(Some(LA_1300), false)
    );
    let out = format!("{SESSION}\nThe answer continues here.\n");
    assert_eq!(limit_message(b"", out.as_bytes(), NOW), None);
}

#[test]
fn model_prose_about_usage_limits_is_not_a_limit_message() {
    let prose = "The API has a usage limit. You've hit your daily usage limit for tests, so \
                 the session limit resets 8:30am (America/Los_Angeles).\n\
                 Claude AI usage limit reached in the docs example above.";
    assert_eq!(limit_message(b"", prose.as_bytes(), NOW), None);
    assert_eq!(limit_message(prose.as_bytes(), b"", NOW), None);
}

#[test]
fn the_shape_needs_the_prefix_the_word_limit_and_the_dot() {
    for line in [
        "You've hit your session limit, resets 1pm (America/Los_Angeles)",
        "You've hit your stride · well done",
        "  note: You've hit your session limit · resets 1pm (UTC)",
    ] {
        assert_eq!(limit_message(line.as_bytes(), b"", NOW), None, "{line}");
    }
    let curly = SESSION.replace('\'', "\u{2019}");
    assert_eq!(
        limit_message(curly.as_bytes(), b"", NOW),
        msg(Some(LA_1300), false)
    );
}

#[test]
fn the_epoch_form_states_its_reset_and_a_message_with_no_reset_says_so() {
    let line = epoch_line("1792337400");
    assert_eq!(
        limit_message(line.as_bytes(), b"", NOW),
        msg(Some(LA_0830), false)
    );
    let none = "You've hit your session limit · try again later";
    assert_eq!(limit_message(none.as_bytes(), b"", NOW), msg(None, false));
}

#[test]
fn limit_message_survives_invalid_utf8_and_huge_input() {
    let mut junk = vec![0xffu8; 70 * 1024];
    junk.extend_from_slice(b"\nYou've hit your limit \xc2\xb7 resets \xe2\x82");
    for now in [NOW, i64::MAX, i64::MIN] {
        let _ = limit_message(&junk, &junk, now);
    }
}

#[test]
fn reset_hint_quotes_the_text_after_the_anchor_as_today() {
    assert_eq!(
        reset_hint("You've hit your limit · resets 9:20am (Europe/Paris)\nmore").as_deref(),
        Some("9:20am (Europe/Paris)")
    );
    assert_eq!(reset_hint("RESETS soon").as_deref(), Some("soon"));
    assert_eq!(reset_hint("resets   \r\nx"), None);
    assert_eq!(reset_hint("no anchor"), None);
    assert_eq!(
        reset_hint("resets 8:30am (America/Los_Angeles)").as_deref(),
        super::failure_class::usage_limit_reset_hint("resets 8:30am (America/Los_Angeles)")
            .as_deref(),
        "the classifier's hint is a thin wrapper"
    );
}
