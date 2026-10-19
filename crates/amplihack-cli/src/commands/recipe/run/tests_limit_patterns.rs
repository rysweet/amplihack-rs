//! `limit_patterns` (issue #1618): the one table of usage-limit patterns per
//! harness, the epoch and text reset readers, the five-hour cap for the wait
//! and the operator-facing reset hint.
//!
//! Wired from `recipe/run/mod.rs` with `#[cfg(test)] mod tests_limit_patterns;`.

use super::limit_patterns::{Harness, all_markers, detect, epoch_reset, read_reset, reset_hint};
use super::{FailureClass, classify_failure_text};

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;
/// 2026-10-18 08:30:00 -07:00.
const LA_0830: i64 = 1_792_337_400;
const FIVE_HOURS: i64 = 5 * 3600;

/// Claude's epoch-form marker, built so this file does not hold the literal
/// either (the pattern-home guard skips `tests_*.rs`, but keep it obvious).
fn epoch_line(rest: &str) -> String {
    format!("Claude AI usage limit {}|{rest}", "reached")
}

const DOCUMENTED_MARKERS: [&str; 8] = [
    "session limit",
    "usage limit",
    "usage_limit_reached",
    "quota exceeded",
    "monthly limit",
    "weekly limit",
    "spend limit",
    "credit balance is too low",
];

#[test]
fn claude_is_the_only_harness_and_is_named_claude() {
    assert_eq!(Harness::ALL, &[Harness::Claude]);
    assert_eq!(Harness::Claude.as_str(), "claude");
}

#[test]
fn all_markers_is_exactly_the_documented_list() {
    let mut got: Vec<&str> = all_markers().into_iter().collect();
    got.sort_unstable();
    let mut want = DOCUMENTED_MARKERS.to_vec();
    want.sort_unstable();
    assert_eq!(got, want);
    for m in got {
        assert_eq!(m, m.to_ascii_lowercase(), "markers are matched lowercased");
    }
}

#[test]
fn every_marker_still_classifies_as_a_usage_limit() {
    // `credit balance is too low` is a marker, but the environmental check
    // runs first and claims it; that precedence is unchanged.
    for m in DOCUMENTED_MARKERS
        .into_iter()
        .filter(|m| *m != "credit balance is too low")
    {
        let text = format!("Notice: {m} reached. Try again later.");
        assert_eq!(
            classify_failure_text(&text).0,
            FailureClass::UsageLimit,
            "marker {m:?}"
        );
    }
}

#[test]
fn detect_names_the_harness_whose_marker_appears() {
    assert_eq!(
        detect("You've hit your usage limit · resets 8:30am (America/Los_Angeles)"),
        Some(Harness::Claude)
    );
    assert_eq!(detect("SESSION LIMIT reached"), Some(Harness::Claude));
    assert_eq!(detect(&epoch_line("1792337400")), Some(Harness::Claude));
    assert_eq!(detect("API Error: 429 rate limit"), None);
    assert_eq!(detect("daily limit"), None);
    assert_eq!(detect(""), None);
}

#[test]
fn epoch_reset_reads_the_last_marker_line_of_stderr() {
    let stderr = format!(
        "{}\nworking\n{}\n",
        epoch_line("1792336000"),
        epoch_line("1792337400")
    );
    assert_eq!(epoch_reset(stderr.as_bytes(), b""), Some(LA_0830));
}

#[test]
fn epoch_reset_falls_back_to_stdout() {
    let stdout = format!("partial\n{}\n", epoch_line("1792337400"));
    assert_eq!(
        epoch_reset(b"no marker here\n", stdout.as_bytes()),
        Some(LA_0830)
    );
}

#[test]
fn epoch_reset_is_not_clamped() {
    let far = (NOW + 100 * 86_400).to_string();
    assert_eq!(
        epoch_reset(epoch_line(&far).as_bytes(), b""),
        Some(NOW + 100 * 86_400)
    );
    assert_eq!(epoch_reset(epoch_line("1").as_bytes(), b""), Some(1));
}

#[test]
fn epoch_reset_accepts_one_to_nineteen_digits_only() {
    assert_eq!(epoch_reset(epoch_line("7").as_bytes(), b""), Some(7));
    assert_eq!(
        epoch_reset(epoch_line("9223372036854775807").as_bytes(), b""),
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
            epoch_reset(epoch_line(bad).as_bytes(), b""),
            None,
            "suffix {bad:?}"
        );
    }
}

#[test]
fn epoch_reset_ignores_trailing_text_after_the_digits() {
    assert_eq!(
        epoch_reset(epoch_line("1792337400 (local)").as_bytes(), b""),
        Some(LA_0830)
    );
}

#[test]
fn read_reset_reads_the_text_form_from_stderr_or_stdout() {
    let msg = "You've hit your usage limit · resets 8:30am (America/Los_Angeles)\n";
    assert_eq!(read_reset(msg.as_bytes(), b"", NOW), Some(LA_0830));
    assert_eq!(read_reset(b"exit\n", msg.as_bytes(), NOW), Some(LA_0830));
}

#[test]
fn read_reset_prefers_the_epoch_form() {
    let stderr = format!(
        "{}\nYou've hit your usage limit · resets 9:30am (America/Los_Angeles)\n",
        epoch_line("1792337400")
    );
    assert_eq!(read_reset(stderr.as_bytes(), b"", NOW), Some(LA_0830));
}

#[test]
fn read_reset_caps_at_five_hours_from_now() {
    let far = (NOW + 40_000).to_string();
    assert_eq!(
        read_reset(epoch_line(&far).as_bytes(), b"", NOW),
        Some(NOW + FIVE_HOURS)
    );
    // 07:45 is 15 minutes past, so tomorrow, then capped.
    let msg = b"session limit reached \xc2\xb7 resets 7:45am (America/Los_Angeles)";
    assert_eq!(read_reset(msg, b"", NOW), Some(NOW + FIVE_HOURS));
}

#[test]
fn read_reset_keeps_a_past_reset_as_read() {
    assert_eq!(
        read_reset(epoch_line("1792330000").as_bytes(), b"", NOW),
        Some(1_792_330_000),
        "a reset in the past is not raised to now"
    );
    let grace = b"usage limit \xc2\xb7 resets 7:55am (America/Los_Angeles)";
    assert_eq!(read_reset(grace, b"", NOW), Some(NOW - 300));
}

#[test]
fn read_reset_is_none_when_no_form_parses() {
    for stderr in [
        "Claude usage limit reached for this week".to_string(),
        "weekly limit · resets Oct 20 at 9am (UTC)".to_string(),
        "usage limit · resets 8:30am".to_string(),
        epoch_line("soon"),
    ] {
        assert_eq!(read_reset(stderr.as_bytes(), b"", NOW), None, "{stderr:?}");
    }
}

#[test]
fn read_reset_survives_invalid_utf8_and_huge_input() {
    let mut junk = vec![0xffu8; 70 * 1024];
    junk.extend_from_slice(b"\nresets \xe2\x82");
    let _ = read_reset(&junk, &junk, NOW);
    let _ = read_reset(&junk, &junk, i64::MAX);
    let _ = read_reset(&junk, &junk, i64::MIN);
    let far = epoch_line("9223372036854775807");
    assert_eq!(
        read_reset(far.as_bytes(), b"", i64::MAX - 10),
        Some(i64::MAX),
        "the cap saturates instead of overflowing"
    );
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
