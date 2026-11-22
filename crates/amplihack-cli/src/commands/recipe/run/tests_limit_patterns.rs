//! `limit_patterns` (issue #1618): the markers the classifier reads, the
//! epoch reader the auto-switch ledger uses, the shaped limit-message reader
//! the usage-limit wait uses (with messages seen on real hosts), and the
//! operator-facing reset hint.
//!
//! Wired from `recipe/run/mod.rs` with `#[cfg(test)] mod tests_limit_patterns;`.

use super::failure_markers::TRANSIENT_MARKERS;
use super::limit_patterns::{
    CLAUDE_RATE_LIMIT, Harness, LimitKind, LimitMessage, all_markers, epoch_marker, limit_message,
    rate_limit, reset_hint,
};
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

/// A Claude message stating `reset`. `spend` picks the kind this file's
/// own fixtures carry: every non-spend line here names the session or a
/// per-model window, which is [`LimitKind::Session`]. The kinds themselves
/// are covered in `tests_limit_kind.rs`.
fn msg(reset: Option<i64>, spend: bool) -> Option<LimitMessage> {
    Some(LimitMessage {
        harness: Harness::Claude,
        kind: if spend {
            LimitKind::Spend
        } else {
            LimitKind::Session
        },
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
    assert!(!weekly.spend());
    assert!(weekly.reset.is_some_and(|r| r > NOW + 300 * 86_400));
}

#[test]
fn a_spend_limit_line_carries_the_session_or_weekly_reset_it_states() {
    let weekly = limit_message(SPEND_WEEKLY.as_bytes(), b"", NOW).unwrap();
    assert!(weekly.spend());
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

// Issue #2230: Claude Code now names the session window "5-hour limit".

/// The line from #2230.
const FIVE_HOUR: &str = "You've hit your 5-hour limit · resets 8:20pm (America/Los_Angeles)";
/// 2026-10-18 20:20:00 -07:00, the reset FIVE_HOUR states at NOW.
const LA_2020: i64 = 1_792_380_000;
/// 2026-10-18 21:00:00 -07:00, past 20:20 and its 10-minute grace.
const LA_2100: i64 = 1_792_382_400;

#[test]
fn the_five_hour_limit_message_is_read_from_stderr() {
    assert_eq!(
        limit_message(FIVE_HOUR.as_bytes(), b"", NOW),
        msg(Some(LA_2020), false)
    );
}

#[test]
fn a_per_model_limit_message_is_read_from_stderr() {
    let opus = "You've hit your Opus limit · resets 8:20pm (America/Los_Angeles)";
    assert_eq!(
        limit_message(opus.as_bytes(), b"", NOW),
        msg(Some(LA_2020), false)
    );
}

#[test]
fn the_five_hour_limit_message_is_read_from_the_last_line_of_stdout() {
    let out = format!("attempt output\n{FIVE_HOUR}\n\n");
    assert_eq!(
        limit_message(b"", out.as_bytes(), NOW),
        msg(Some(LA_2020), false)
    );
}

#[test]
fn the_five_hour_limit_message_with_a_curly_apostrophe_is_read() {
    let curly = FIVE_HOUR.replace('\'', "\u{2019}");
    assert_eq!(
        limit_message(curly.as_bytes(), b"", NOW),
        msg(Some(LA_2020), false)
    );
    assert_eq!(
        classify_failure_text(&curly),
        (FailureClass::UsageLimit, Some("5-hour limit"))
    );
}

#[test]
fn a_five_hour_reset_already_passed_today_is_tomorrows() {
    assert_eq!(
        limit_message(FIVE_HOUR.as_bytes(), b"", LA_2100),
        msg(Some(LA_2020 + 86_400), false)
    );
}

#[test]
fn reset_hint_quotes_the_five_hour_reset() {
    assert_eq!(
        reset_hint(FIVE_HOUR).as_deref(),
        Some("8:20pm (America/Los_Angeles)")
    );
}

#[test]
fn prose_naming_a_model_limit_is_not_a_limit_message() {
    let out = "Here is the answer.\nThe Opus limit is documented here\n";
    assert_eq!(limit_message(b"", out.as_bytes(), NOW), None);
    assert_eq!(limit_message(out.as_bytes(), b"", NOW), None);
}

// --- Issue #2857: the 429 the retry after a wait may meet ------------------------
//
// `rate_limit(stderr, stdout) -> bool` reads a harness's 429 envelope at one
// position only, the one `limit_message` reads a usage-limit message at: a
// whole line on stderr, or the last non-empty line of the stdout tail. It is
// a separate table, not a marker added to the usage-limit list:
// `TRANSIENT_MARKERS` already holds these phrases so the recipe runner calls a
// 429 a transient transport fault that clears in seconds, and widening the
// usage-limit markers would make every transient 429 in the runner, and in
// auto-drive's terminal decisions, a usage limit that waits for hours.
//
// The position is the whole rule. The same words mid-line, or in an earlier
// line of model output, are the model's own prose.

const API_429: &str = r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed your account's rate limit."}}"#;

#[test]
fn a_429_line_on_stderr_is_a_rate_limit() {
    for line in [
        API_429,
        "429 Too Many Requests",
        "rate_limit_error",
        "rate limit exceeded",
        "  API Error: 429 Too Many Requests  ",
        "API ERROR: 429",
        "Rate Limit Exceeded",
    ] {
        assert!(rate_limit(line.as_bytes(), b""), "{line:?}");
    }
}

#[test]
fn the_last_line_of_stdout_counts_when_it_is_the_429() {
    let out = format!("attempt output\n{API_429}\n\n");
    assert!(rate_limit(b"", out.as_bytes()));
    // Surrounding blank lines are not lines.
    assert!(rate_limit(b"", b"\n\n  429 Too Many Requests \n\n"));
}

#[test]
fn a_429_phrase_mid_line_or_in_earlier_output_is_the_models_prose() {
    for prose in [
        "The build failed: API Error: 429 was in the log, retrying",
        "got a 429 Too Many Requests from the proxy, backing off",
        "I will handle rate_limit_error in the client",
        "the rate limit exceeded message is documented here",
    ] {
        assert!(!rate_limit(prose.as_bytes(), b""), "stderr: {prose:?}");
        assert!(!rate_limit(b"", prose.as_bytes()), "stdout: {prose:?}");
    }
    // A 429 as an earlier line of stdout, with the model's answer after it.
    let out = format!("{API_429}\nSorry, I could not finish that.\n");
    assert!(!rate_limit(b"", out.as_bytes()));
    // The same line as the last one is the control for the case above.
    let out = format!("Sorry, I could not finish that.\n{API_429}\n");
    assert!(rate_limit(b"", out.as_bytes()));
}

/// A stderr line wins wherever it is among stderr's lines, as a usage-limit
/// message does: the harness writes its own envelope there.
#[test]
fn a_429_line_on_stderr_counts_among_other_stderr_lines() {
    let err = format!("warning: slow network\n{API_429}\nretrying\n");
    assert!(rate_limit(err.as_bytes(), b""));
}

#[test]
fn nothing_and_unrelated_output_is_not_a_rate_limit() {
    assert!(!rate_limit(b"", b""));
    assert!(!rate_limit(b"attempt output\n", b"all done\n"));
    assert!(!rate_limit(b"error: 4290 widgets\n", b""));
    // A usage-limit message is a different thing, read by `limit_message`.
    assert!(!rate_limit(SESSION.as_bytes(), b""));
}

#[test]
fn rate_limit_survives_invalid_utf8_and_huge_input() {
    let mut junk = vec![0xffu8; 70 * 1024];
    junk.extend_from_slice(b"\n429 Too Many Requests");
    let _ = rate_limit(&junk, b"");
    assert!(
        rate_limit(b"", &junk),
        "the last line of a huge tail still counts"
    );
    let _ = rate_limit(&junk, &junk);
}

/// Where this matters most. A 429 is still the transient transport fault the
/// recipe runner's classifier calls it, and the usage-limit wait's own reader
/// of messages still ignores it: nothing but the one position in the
/// supervisor reads a 429 as a limit.
#[test]
fn a_429_is_still_transient_and_never_a_usage_limit_message() {
    for line in [
        API_429,
        "429 Too Many Requests",
        "rate_limit_error",
        "rate limit exceeded",
    ] {
        assert_eq!(
            classify_failure_text(line).0,
            FailureClass::TransientTransport,
            "{line:?}"
        );
        assert_ne!(classify_failure_text(line).0, FailureClass::UsageLimit);
        assert_eq!(limit_message(line.as_bytes(), b"", NOW), None, "{line:?}");
        assert_eq!(epoch_marker(line.as_bytes(), b""), None, "{line:?}");
    }
    for m in super::limit_patterns::all_markers() {
        let m = m.to_ascii_lowercase();
        assert!(
            !m.contains("429") && !m.contains("rate_limit") && !m.contains("rate limit"),
            "{m:?}: a 429 phrase was added to the usage-limit markers"
        );
    }
}

// --- Issue #2857: the 429 phrases are one list ---------------------------------------
//
// `CLAUDE_RATE_LIMIT` is where the phrases are defined. The recipe runner's
// transient markers are composed from it, so the position-anchored reader that
// part 1 uses and the classifier that calls a 429 a transient transport fault
// cannot drift apart, and no comment has to ask an editor to change both.
//
// What the list must never become is a usage-limit marker. A 429 clears in
// seconds; a usage limit waits hours. Widening the table's visibility for the
// transient list must not let one reach `all_markers()`.

#[test]
fn every_429_phrase_is_a_transient_marker() {
    assert!(!CLAUDE_RATE_LIMIT.is_empty());
    for phrase in CLAUDE_RATE_LIMIT {
        assert!(
            TRANSIENT_MARKERS.contains(phrase),
            "{phrase:?} is read as a 429 and is not a transient marker"
        );
    }
}

/// SEC-6. Not a member, and not contained in a member's text either way round:
/// a usage-limit marker inside a 429 phrase would classify that line as a limit.
#[test]
fn no_429_phrase_is_a_usage_limit_marker() {
    for phrase in CLAUDE_RATE_LIMIT {
        for marker in all_markers() {
            let (phrase, marker) = (phrase.to_ascii_lowercase(), marker.to_ascii_lowercase());
            assert!(
                !phrase.contains(&marker) && !marker.contains(&phrase),
                "{phrase:?} and the usage-limit marker {marker:?} overlap"
            );
        }
    }
}

/// Composing the list from two parts must not list a phrase twice, which is
/// what a half-finished collapse leaves behind.
#[test]
fn the_transient_markers_list_no_phrase_twice() {
    let mut seen = std::collections::HashSet::new();
    for marker in TRANSIENT_MARKERS.iter() {
        assert!(seen.insert(*marker), "{marker:?} is listed twice");
    }
}

/// `first_match` picks the earliest marker, and the longest at a tie, so the
/// order the parts are joined in does not matter. These lines pin what each 429
/// phrase is classified by, which is what the evidence reports to the operator.
#[test]
fn the_marker_a_429_line_is_classified_by_is_the_one_that_starts_it() {
    for (line, marker) in [
        (API_429, "api error: 429"),
        ("429 Too Many Requests", "429 too many requests"),
        ("rate_limit_error", "rate_limit_error"),
        ("rate limit exceeded", "rate limit exceeded"),
    ] {
        assert_eq!(
            classify_failure_text(line),
            (FailureClass::TransientTransport, Some(marker)),
            "{line:?}"
        );
    }
}
