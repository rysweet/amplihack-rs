//! Usage meter: status-line parsing, the fingerprint, attribution after a
//! swap, merging readings, and the usage-limit rerun decision (issue #1569).

use super::rerun;
use super::state::HostEntry;
use super::usage::{
    self, Attribution, COOLDOWN_SECS, IgnoreReason, RateLimits, Reading, ReadingSource, Window,
};
use sha2::{Digest, Sha256};

const NOW: i64 = 1_760_000_000;
const FIVE_HOURS: i64 = 5 * 3600;

fn input(body: &str) -> Vec<u8> {
    format!("{{\"session_id\":\"s\",\"rate_limits\":{body}}}").into_bytes()
}

fn win(pct: f64, resets_at: i64) -> Window {
    Window {
        used_percentage: pct,
        resets_at,
    }
}

#[test]
fn parses_epoch_seconds_integer_and_float() {
    let raw = input(&format!(
        "{{\"five_hour\":{{\"used_percentage\":93,\"resets_at\":{}}},\
         \"seven_day\":{{\"used_percentage\":61.5,\"resets_at\":{}.75}}}}",
        NOW + 600,
        NOW + 86_400
    ));
    let rl = usage::parse_statusline_input(&raw, NOW).unwrap();
    assert_eq!(rl.five_hour, Some(win(93.0, NOW + 600)));
    assert_eq!(rl.seven_day, Some(win(61.5, NOW + 86_400)));
}

#[test]
fn parses_rfc3339_reset_times() {
    // 1_760_000_000 = 2025-10-09T08:53:20Z
    let raw =
        input("{\"five_hour\":{\"used_percentage\":10,\"resets_at\":\"2025-10-09T09:53:20Z\"}}");
    let rl = usage::parse_statusline_input(&raw, NOW).unwrap();
    assert_eq!(rl.five_hour, Some(win(10.0, NOW + 3600)));
    assert_eq!(rl.seven_day, None);
}

#[test]
fn missing_rate_limits_or_malformed_input_is_none() {
    assert!(usage::parse_statusline_input(b"{\"model\":{}}", NOW).is_none());
    assert!(usage::parse_statusline_input(b"not json", NOW).is_none());
    assert!(usage::parse_statusline_input(b"", NOW).is_none());
    assert!(
        usage::parse_statusline_input(&input("{}"), NOW).is_none(),
        "no windows"
    );
}

#[test]
fn input_over_one_mebibyte_is_refused() {
    let mut raw = input(&format!(
        "{{\"five_hour\":{{\"used_percentage\":1,\"resets_at\":{}}}}}",
        NOW + 60
    ));
    raw.truncate(raw.len() - 1);
    raw.extend_from_slice(b",\"pad\":\"");
    raw.extend(std::iter::repeat_n(b'a', 1 << 20));
    raw.extend_from_slice(b"\"}");
    assert!(usage::parse_statusline_input(&raw, NOW).is_none());
}

#[test]
fn non_finite_or_out_of_range_percentages() {
    // JSON cannot carry NaN; a string or a huge number must not slip through.
    let raw = input(&format!(
        "{{\"five_hour\":{{\"used_percentage\":\"NaN\",\"resets_at\":{}}},\
         \"seven_day\":{{\"used_percentage\":1e400,\"resets_at\":{}}}}}",
        NOW + 60,
        NOW + 60
    ));
    let rl = usage::parse_statusline_input(&raw, NOW);
    assert!(rl.is_none_or(|r| r.five_hour.is_none() && r.seven_day.is_none()));

    let raw = input(&format!(
        "{{\"five_hour\":{{\"used_percentage\":150,\"resets_at\":{}}},\
         \"seven_day\":{{\"used_percentage\":-3,\"resets_at\":{}}}}}",
        NOW + 60,
        NOW + 60
    ));
    let rl = usage::parse_statusline_input(&raw, NOW).unwrap();
    assert_eq!(rl.five_hour.unwrap().used_percentage, 100.0, "clamped");
    assert_eq!(rl.seven_day.unwrap().used_percentage, 0.0, "clamped");
}

#[test]
fn reset_times_outside_minus_one_to_plus_eight_days_drop_the_window() {
    for resets_at in [NOW - 86_401, NOW + 8 * 86_400 + 1, i64::MAX, i64::MIN] {
        let raw = input(&format!(
            "{{\"five_hour\":{{\"used_percentage\":5,\"resets_at\":{resets_at}}},\
             \"seven_day\":{{\"used_percentage\":6,\"resets_at\":{}}}}}",
            NOW + 60
        ));
        let rl = usage::parse_statusline_input(&raw, NOW).unwrap();
        assert_eq!(rl.five_hour, None, "resets_at={resets_at}");
        assert!(rl.seven_day.is_some());
    }
    let raw = input("{\"five_hour\":{\"used_percentage\":5,\"resets_at\":1e300}}");
    assert!(usage::parse_statusline_input(&raw, NOW).is_none_or(|r| r.five_hour.is_none()));
}

#[test]
fn fingerprint_is_full_sha256_of_the_documented_text() {
    let rl = RateLimits {
        five_hour: Some(win(93.0, 100)),
        seven_day: Some(win(61.5, 200)),
    };
    let want = hex(&Sha256::digest(b"93.0|100|61.5|200"));
    let got = usage::fingerprint(&rl);
    assert_eq!(got.len(), 64);
    assert!(
        got.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(got, want);

    let missing = RateLimits {
        five_hour: None,
        seven_day: Some(win(1.0, 2)),
    };
    assert_eq!(
        usage::fingerprint(&missing),
        hex(&Sha256::digest(b"-|-|1.0|2"))
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn limits(five: f64) -> RateLimits {
    RateLimits {
        five_hour: Some(win(five, NOW + 3600)),
        seven_day: Some(win(10.0, NOW + 86_400)),
    }
}

#[test]
fn attribution_without_a_swap_goes_to_the_owner() {
    assert_eq!(
        usage::attribute("X", None, &limits(50.0), NOW),
        Attribution::Record("X".into())
    );
}

#[test]
fn attribution_after_a_swap_goes_to_the_live_account() {
    let host = HostEntry {
        live: "Y".into(),
        swapped_at: NOW - COOLDOWN_SECS - 1,
        pre_swap_fingerprint: Some(usage::fingerprint(&limits(95.0))),
    };
    assert_eq!(
        usage::attribute("X", Some(&host), &limits(12.0), NOW),
        Attribution::Record("Y".into())
    );
}

#[test]
fn readings_inside_the_cooldown_are_ignored() {
    let host = HostEntry {
        live: "Y".into(),
        swapped_at: NOW - COOLDOWN_SECS + 1,
        pre_swap_fingerprint: None,
    };
    assert_eq!(COOLDOWN_SECS, 300);
    assert_eq!(
        usage::attribute("X", Some(&host), &limits(12.0), NOW),
        Attribution::Ignore(IgnoreReason::Cooldown)
    );
}

#[test]
fn reading_identical_to_the_pre_swap_one_is_ignored_after_the_cooldown() {
    let before = limits(95.0);
    let host = HostEntry {
        live: "Y".into(),
        swapped_at: NOW - 3600,
        pre_swap_fingerprint: Some(usage::fingerprint(&before)),
    };
    assert_eq!(
        usage::attribute("X", Some(&host), &before, NOW),
        Attribution::Ignore(IgnoreReason::Fingerprint)
    );
}

fn reading(five: f64, observed_at: i64) -> Reading {
    Reading {
        five_hour: Some(win(five, NOW + 3600)),
        seven_day: None,
        observed_at,
        source: ReadingSource::Statusline,
    }
}

#[test]
fn merge_keeps_the_newer_reading_and_the_higher_on_a_tie() {
    let old = reading(80.0, NOW - 10);
    let new = reading(20.0, NOW);
    assert_eq!(usage::merge(Some(&old), new.clone()), new);
    assert_eq!(
        usage::merge(Some(&new), old.clone()),
        new,
        "older never wins"
    );
    let tie_low = reading(30.0, NOW);
    let tie_high = reading(70.0, NOW);
    assert_eq!(usage::merge(Some(&tie_low), tie_high.clone()), tie_high);
    assert_eq!(usage::merge(Some(&tie_high), tie_low), tie_high);
    assert_eq!(usage::merge(None, old.clone()), old);
}

// ---- usage-limit reruns: the decision (D2) and the reset time ------------

const MARKER: &str = "Claude AI usage limit reached";

fn limited(stderr: &str, stdout: &str, code: i32, signalled: bool) -> Option<i64> {
    rerun::limit_reset(signalled, code, stderr.as_bytes(), stdout.as_bytes(), NOW)
}

#[test]
fn marker_followed_by_another_stderr_line_reruns() {
    // (a) The case the old last-line rule missed while the runner ended the run.
    let stderr = format!("{MARKER}|{}\nError: request failed\n", NOW + 600);
    assert_eq!(limited(&stderr, "", 1, false), Some(NOW + 600));
}

#[test]
fn marker_with_exit_zero_never_reruns() {
    // (b) A successful step that merely prints the text.
    let stderr = format!("{MARKER}|{}\n", NOW + 600);
    assert_eq!(limited(&stderr, "", 0, false), None);
    assert_eq!(limited("", &stderr, 0, false), None);
}

#[test]
fn marker_with_a_work_or_environment_signal_never_reruns() {
    // (c) Work and environmental markers outrank a usage limit.
    for extra in [
        "test result: FAILED. 1 passed; 1 failed",
        "error[E0425]: cannot find value",
        "Permission denied (publickey)",
        "Credit balance is too low",
        "disk quota exceeded",
    ] {
        let stderr = format!("{MARKER}|{}\n{extra}\n", NOW + 600);
        assert_eq!(limited(&stderr, "", 1, false), None, "{extra}");
    }
}

#[test]
fn marker_without_a_parsable_time_resets_in_five_hours() {
    // (d)
    for stderr in [
        format!("{MARKER}. Try again later.\n"),
        format!("{MARKER}|soon\n"),
        "Error: usage limit reached for this account\n".to_string(),
    ] {
        assert_eq!(
            limited(&stderr, "", 1, false),
            Some(NOW + FIVE_HOURS),
            "{stderr}"
        );
    }
}

#[test]
fn killed_by_a_signal_never_reruns() {
    // (e)
    let stderr = format!("{MARKER}|{}\n", NOW + 600);
    assert_eq!(limited(&stderr, "", 137, true), None);
}

#[test]
fn stdout_tail_counts_too() {
    let stdout = format!("partial\n{MARKER}|{}\n", NOW + 900);
    assert_eq!(limited("", &stdout, 1, false), Some(NOW + 900));
}

#[test]
fn output_without_a_usage_limit_never_reruns() {
    for stderr in [
        "Error: something else failed\n",
        "API Error: 529 overloaded\n",
        "",
    ] {
        assert_eq!(limited(stderr, "", 1, false), None, "{stderr:?}");
    }
}

#[test]
fn reset_time_comes_from_a_line_that_starts_with_the_marker() {
    let stderr = format!("working\n{MARKER}|{}\nmore\n\n", NOW + 600);
    assert_eq!(
        usage::usage_limit_reset(stderr.as_bytes(), b"", NOW),
        Some(NOW + 600)
    );
    for stderr in [
        format!("model: {MARKER}|{}\n", NOW + 600),
        format!(" {MARKER}|{}\n", NOW + 600),
        format!("{}|{}\n", MARKER.to_lowercase(), NOW + 600),
        "usage limit reached\n".to_string(),
    ] {
        assert_eq!(
            usage::usage_limit_reset(stderr.as_bytes(), b"", NOW),
            None,
            "{stderr:?} supplies no reset time"
        );
        assert_eq!(
            usage::reset_or_default(stderr.as_bytes(), b"", NOW),
            NOW + FIVE_HOURS
        );
    }
}

#[test]
fn the_last_marker_line_wins_and_stderr_before_stdout() {
    let stderr = format!("{MARKER}|{}\n{MARKER}|{}\n", NOW + 100, NOW + 200);
    let stdout = format!("{MARKER}|{}\n", NOW + 300);
    assert_eq!(
        usage::usage_limit_reset(stderr.as_bytes(), stdout.as_bytes(), NOW),
        Some(NOW + 200)
    );
    assert_eq!(
        usage::usage_limit_reset(b"no marker\n", stdout.as_bytes(), NOW),
        Some(NOW + 300)
    );
}

#[test]
fn usage_limit_reset_is_never_later_than_five_hours() {
    let stderr = format!("{MARKER}|{}\n", NOW + 30 * 86_400);
    assert_eq!(
        usage::usage_limit_reset(stderr.as_bytes(), b"", NOW),
        Some(NOW + FIVE_HOURS)
    );
    let stderr = format!("{MARKER}|99999999999999999999999\n");
    assert_eq!(
        usage::usage_limit_reset(stderr.as_bytes(), b"", NOW),
        Some(NOW + FIVE_HOURS)
    );
}
