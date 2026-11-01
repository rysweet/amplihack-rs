//! `usage_request` (issue #1698): the `get_usage` request line and argv, and
//! the reader of the answer.
//!
//! Contract:
//! - `REQUEST_LINE` is one JSON object of 113 bytes, `ARGS` the stream-json
//!   argv; both are quoted in the reference and checked by the docs-sync
//!   tests.
//! - `parse_answer(stdout, now)` takes the first line whose `type` is
//!   `control_response` and whose `response.request_id` is
//!   `amplihack-usage`. It accepts it only when `response.subtype` is
//!   `success`, `response.response.rate_limits_available` is `true`,
//!   `rate_limits` is an object, usage credits are off at both places, and
//!   `five_hour` or `seven_day` gives a window. Anything else is `None`,
//!   which means fall back to `/usage`.
//! - Utilization is used as read when finite and from 0 to 1,000,000;
//!   otherwise that window is left out, never read as 0.
//! - Text (`resets_at`, `display_name`) is used only when 1 to 64
//!   characters with no control character and nothing in U+202A to U+202E
//!   or U+2066 to U+2069.
//!
//! Every input is a string; nothing runs `claude`.

use super::usage_cmd::format_pct;
use super::usage_parse::{UsageReport, UsageState, UsageWindow};
use super::usage_request::{ARGS, REQUEST_ID, REQUEST_LINE, parse_answer};
use serde_json::{Value, json};

/// 2026-10-16T12:00:00Z.
const NOW: i64 = 1_792_152_000;
const SESSION_RESET: &str = "2026-10-16T17:00:00Z";
const SESSION_AT: i64 = 1_792_170_000;
const WEEK_RESET: &str = "2026-10-22T09:00:00Z";
const WEEK_AT: i64 = 1_792_659_600;
const RLO: char = '\u{202e}';

/// A payload shaped as Claude Code 2.1.314 answers, with invented figures:
/// session 37%, week 61%, one model entry, usage credits off.
fn payload() -> Value {
    json!({
        "rate_limits_available": true,
        "rate_limits": {
            "five_hour": {"utilization": 37.0, "resets_at": SESSION_RESET},
            "seven_day": {"utilization": 61.0, "resets_at": WEEK_RESET},
            "seven_day_opus": {"utilization": 99.0, "resets_at": WEEK_RESET},
            "seven_day_sonnet": {"utilization": 98.0, "resets_at": WEEK_RESET},
            "seven_day_oauth_apps": {"utilization": 97.0, "resets_at": WEEK_RESET},
            "model_scoped": [
                {"display_name": "Sonnet", "utilization": 12.0, "resets_at": WEEK_RESET}
            ],
            "extra_usage": {
                "is_enabled": false,
                "monthly_limit": null,
                "used_credits": 0,
                "utilization": null,
                "currency": "USD"
            }
        }
    })
}

/// The answer envelope (C1) around `payload`, as one line.
fn answer(payload: Value) -> String {
    envelope("success", REQUEST_ID, payload)
}

fn envelope(subtype: &str, id: &str, payload: Value) -> String {
    json!({
        "type": "control_response",
        "response": {"subtype": subtype, "request_id": id, "response": payload}
    })
    .to_string()
}

fn parse(stdout: &str) -> Option<UsageReport> {
    parse_answer(stdout, NOW)
}

/// The answer to [`payload`] after `edit`, as the only line.
fn with(edit: impl FnOnce(&mut Value)) -> Option<UsageReport> {
    let mut p = payload();
    edit(&mut p);
    parse(&answer(p))
}

fn limits(p: &mut Value) -> &mut Value {
    &mut p["rate_limits"]
}

fn window(label: &str, pct: f64, text: &str, at: i64) -> UsageWindow {
    UsageWindow {
        label: label.to_string(),
        used_percentage: pct,
        resets_text: Some(text.to_string()),
        resets_at: Some(at),
    }
}

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

#[test]
fn the_request_line_is_one_short_get_usage_request() {
    let v: Value = serde_json::from_str(REQUEST_LINE).unwrap();
    assert_eq!(v["type"], "control_request");
    assert_eq!(v["request_id"], REQUEST_ID);
    assert_eq!(v["request"]["subtype"], "get_usage");
    assert_eq!(v["request"]["skip_behaviors"], true);
    assert_eq!(REQUEST_ID, "amplihack-usage");
    assert_eq!(REQUEST_LINE.len(), 113);
    // With its newline, one write below PIPE_BUF.
    assert!(REQUEST_LINE.len() + 1 < 512);
    assert!(!REQUEST_LINE.contains('\n'));
}

#[test]
fn the_arguments_are_the_stream_json_argv() {
    assert_eq!(
        ARGS.join(" "),
        "-p --input-format stream-json --output-format stream-json --verbose"
    );
}

// ---------------------------------------------------------------------------
// Mapping
// ---------------------------------------------------------------------------

#[test]
fn an_accepted_answer_gives_every_window_with_labels_text_and_epoch() {
    let r = parse(&answer(payload())).expect("accepted");
    assert_eq!(r.state, UsageState::Ok);
    assert_eq!(
        r.session,
        Some(window("session", 37.0, SESSION_RESET, SESSION_AT))
    );
    assert_eq!(
        r.week_all,
        Some(window("all models", 61.0, WEEK_RESET, WEEK_AT))
    );
    // `seven_day_opus`, `seven_day_sonnet` and `seven_day_oauth_apps` are
    // ignored; only `model_scoped` gives model entries.
    assert_eq!(
        r.week_models,
        vec![window("Sonnet", 12.0, WEEK_RESET, WEEK_AT)]
    );
}

#[test]
fn rfc3339_resets_are_read_with_z_an_offset_and_fractions() {
    for text in [
        "2026-10-16T17:00:00Z",
        "2026-10-16T17:00:00+00:00",
        "2026-10-16T17:00:00.123456Z",
        "2026-10-16T19:00:00+02:00",
    ] {
        let r = with(|p| limits(p)["five_hour"]["resets_at"] = json!(text)).expect(text);
        let s = r.session.unwrap();
        assert_eq!(s.resets_at, Some(SESSION_AT), "{text}");
        assert_eq!(s.resets_text.as_deref(), Some(text));
    }
}

#[test]
fn an_answer_with_only_one_window_is_accepted() {
    let r = with(|p| {
        limits(p).as_object_mut().unwrap().remove("seven_day");
    })
    .expect("session alone");
    assert!(r.session.is_some() && r.week_all.is_none());
    let r = with(|p| {
        limits(p).as_object_mut().unwrap().remove("five_hour");
    })
    .expect("week alone");
    assert!(r.session.is_none() && r.week_all.is_some());
}

// ---------------------------------------------------------------------------
// Which line is the answer
// ---------------------------------------------------------------------------

#[test]
fn other_lines_are_skipped() {
    let system = format!(
        r#"{{"type":"system","subtype":"init","tools":["{}"]}}"#,
        "x".repeat(16 * 1024)
    );
    let stdout = format!(
        "{system}\nnot json at all\n{{\"type\":\"result\"}}\n{}\n",
        answer(payload())
    );
    assert!(parse(&stdout).is_some());
}

#[test]
fn an_answer_to_another_request_is_skipped() {
    let other = envelope("error", "someone-else", json!(null));
    let stdout = format!("{other}\n{}\n", answer(payload()));
    assert!(parse(&stdout).is_some());
}

#[test]
fn the_first_matching_line_decides_even_when_rejected() {
    let error = json!({
        "type": "control_response",
        "response": {"subtype": "error", "request_id": REQUEST_ID, "error": "unknown"}
    });
    let stdout = format!("{error}\n{}\n", answer(payload()));
    assert_eq!(parse(&stdout), None);
}

#[test]
fn a_truncated_answer_is_not_read() {
    let line = answer(payload());
    assert_eq!(parse(&line[..line.len() / 2]), None);
    assert_eq!(parse(&line[..line.len() - 1]), None);
}

#[test]
fn a_deeply_nested_line_is_rejected_without_a_panic() {
    let deep = format!(
        r#"{{"type":"control_response","x":{}1{}}}"#,
        "[".repeat(200),
        "]".repeat(200)
    );
    assert_eq!(parse(&deep), None);
    let deep_answer = format!(
        r#"{{"type":"control_response","response":{{"subtype":"success","request_id":"{REQUEST_ID}","response":{}1{}}}}}"#,
        "[".repeat(200),
        "]".repeat(200)
    );
    assert_eq!(parse(&deep_answer), None);
}

#[test]
fn odd_input_never_panics() {
    for s in [
        "",
        "\n\n",
        "control_response",
        "{\"type\":\"control_response\"}",
        "{\"type\":\"control_response\",\"response\":null}",
        "{\"type\":\"control_response\",\"response\":{\"request_id\":\"amplihack-usage\"}}",
        "[\"control_response\"]",
        "\u{feff}{\"type\":\"control_response\"}",
    ] {
        assert_eq!(parse(s), None, "{s:?}");
    }
}

// ---------------------------------------------------------------------------
// Rejection
// ---------------------------------------------------------------------------

#[test]
fn an_error_answer_is_rejected() {
    assert_eq!(parse(&envelope("error", REQUEST_ID, payload())), None);
}

#[test]
fn rate_limits_available_must_be_the_json_true() {
    for v in [json!(false), json!("true"), json!(1), json!(null)] {
        assert_eq!(
            with(|p| p["rate_limits_available"] = v.clone()),
            None,
            "{v}"
        );
    }
    assert_eq!(
        with(|p| {
            p.as_object_mut().unwrap().remove("rate_limits_available");
        }),
        None
    );
}

#[test]
fn rate_limits_must_be_an_object() {
    for v in [json!(null), json!("x"), json!([]), json!(37)] {
        assert_eq!(with(|p| p["rate_limits"] = v.clone()), None, "{v}");
    }
}

#[test]
fn a_payload_that_is_not_an_object_is_rejected() {
    for v in [json!(null), json!("x"), json!([])] {
        assert_eq!(parse(&answer(v.clone())), None, "{v}");
    }
}

#[test]
fn usage_credits_on_at_either_place_are_rejected() {
    let bad = [
        json!({"is_enabled": true}),
        json!({}),
        json!({"is_enabled": "false"}),
        json!({"is_enabled": 0}),
        json!({"is_enabled": null}),
        json!(true),
        json!(false),
        json!("off"),
    ];
    for v in bad {
        assert_eq!(
            with(|p| limits(p)["extra_usage"] = v.clone()),
            None,
            "rate_limits.extra_usage = {v}"
        );
        assert_eq!(
            with(|p| p["extra_usage"] = v.clone()),
            None,
            "extra_usage = {v}"
        );
    }
}

#[test]
fn usage_credits_absent_null_or_off_are_accepted() {
    assert!(
        with(|p| {
            limits(p).as_object_mut().unwrap().remove("extra_usage");
        })
        .is_some()
    );
    assert!(with(|p| limits(p)["extra_usage"] = json!(null)).is_some());
    assert!(with(|p| p["extra_usage"] = json!(null)).is_some());
    assert!(with(|p| p["extra_usage"] = json!({"is_enabled": false})).is_some());
}

// ---------------------------------------------------------------------------
// Utilization
// ---------------------------------------------------------------------------

#[test]
fn a_bad_utilization_leaves_that_window_out() {
    for v in [
        json!(null),
        json!("37"),
        json!(-1),
        json!(-0.5),
        json!(1_000_001),
        json!([37]),
    ] {
        let r = with(|p| limits(p)["five_hour"]["utilization"] = v.clone()).expect("week left");
        assert_eq!(r.session, None, "{v}");
        assert!(r.week_all.is_some());
    }
    let r = with(|p| {
        limits(p)["five_hour"]
            .as_object_mut()
            .unwrap()
            .remove("utilization");
    })
    .unwrap();
    assert_eq!(r.session, None);
}

#[test]
fn utilization_at_the_bounds_is_kept() {
    let r = with(|p| limits(p)["five_hour"]["utilization"] = json!(0)).unwrap();
    assert_eq!(r.session.unwrap().used_percentage, 0.0);
    let r = with(|p| limits(p)["five_hour"]["utilization"] = json!(1_000_000)).unwrap();
    assert_eq!(r.session.unwrap().used_percentage, 1_000_000.0);
}

#[test]
fn both_windows_bad_is_no_answer() {
    assert_eq!(
        with(|p| {
            limits(p)["five_hour"]["utilization"] = json!(null);
            limits(p)["seven_day"]["utilization"] = json!(-3);
        }),
        None
    );
    assert_eq!(
        with(|p| {
            let l = limits(p).as_object_mut().unwrap();
            l.remove("five_hour");
            l.remove("seven_day");
        }),
        None
    );
}

#[test]
fn negative_zero_reads_as_zero() {
    let r = parse(&answer(payload()).replace("37.0", "-0.0")).unwrap();
    let pct = r.session.unwrap().used_percentage;
    assert!(pct == 0.0 && pct.is_sign_positive(), "{pct}");
    assert_eq!(format_pct(pct), "0%");
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[test]
fn the_state_follows_the_session_then_the_week() {
    let state = |s: f64, w: f64| {
        with(|p| {
            limits(p)["five_hour"]["utilization"] = json!(s);
            limits(p)["seven_day"]["utilization"] = json!(w);
        })
        .unwrap()
        .state
    };
    assert_eq!(state(100.0, 10.0), UsageState::SessionExhausted);
    assert_eq!(state(10.0, 100.0), UsageState::WeeklyExhausted);
    assert_eq!(state(100.0, 100.0), UsageState::SessionExhausted);
    assert_eq!(state(99.9, 99.9), UsageState::Ok);
    assert_eq!(state(150.0, 0.0), UsageState::SessionExhausted);
}

// ---------------------------------------------------------------------------
// Resets
// ---------------------------------------------------------------------------

fn session_reset(v: Value) -> UsageWindow {
    with(|p| limits(p)["five_hour"]["resets_at"] = v)
        .unwrap()
        .session
        .unwrap()
}

#[test]
fn a_reset_that_is_not_text_leaves_both_fields_out() {
    // A `resets_at` that is present and not `null` is evidence of a session
    // window (issue #1818), so the window is kept with no reset.
    for v in [json!(1_792_170_000), json!({}), json!(true), json!("")] {
        let s = session_reset(v.clone());
        assert_eq!((s.resets_text, s.resets_at), (None, None), "{v}");
    }
    // A `null` or absent `resets_at` at 0% means no session limit: no window.
    let r = with(|p| {
        limits(p)["five_hour"]["utilization"] = json!(0.0);
        limits(p)["five_hour"]["resets_at"] = json!(null);
    })
    .unwrap();
    assert_eq!(r.session, None);
    assert!(!r.session_limit);
    let r = with(|p| {
        limits(p)["five_hour"]["utilization"] = json!(0.0);
        limits(p)["five_hour"]
            .as_object_mut()
            .unwrap()
            .remove("resets_at");
    })
    .unwrap();
    assert_eq!(r.session, None);
    assert!(!r.session_limit);
}

#[test]
fn text_that_is_not_rfc3339_is_kept_with_no_epoch() {
    let s = session_reset(json!("tomorrow at 5pm"));
    assert_eq!(s.resets_text.as_deref(), Some("tomorrow at 5pm"));
    assert_eq!(s.resets_at, None);
}

#[test]
fn a_reset_more_than_eight_days_ahead_has_no_epoch() {
    // NOW + 8 days exactly is kept; one second later is not.
    let s = session_reset(json!("2026-10-24T12:00:00Z"));
    assert_eq!(s.resets_at, Some(NOW + 8 * 86_400));
    let s = session_reset(json!("2026-10-24T12:00:01Z"));
    assert_eq!(s.resets_text.as_deref(), Some("2026-10-24T12:00:01Z"));
    assert_eq!(s.resets_at, None);
    // A time in the past is kept: the window has reset.
    let s = session_reset(json!("2026-10-16T11:00:00Z"));
    assert_eq!(s.resets_at, Some(NOW - 3600));
}

#[test]
fn text_that_is_too_long_empty_or_unsafe_leaves_both_fields_out() {
    let long = format!("2026-10-16T17:00:00Z{}", " ".repeat(45));
    assert_eq!(long.chars().count(), 65);
    for text in [
        long,
        String::new(),
        "2026-10-16T17:00:00Z\u{1b}[31m".to_string(),
        "2026-10-16\n17:00".to_string(),
        format!("2026-10-16T17:00:00Z{RLO}"),
        "2026-10-16T17:00:00Z\u{2066}".to_string(),
    ] {
        let s = session_reset(json!(text));
        assert_eq!((s.resets_text, s.resets_at), (None, None), "{text:?}");
    }
    // 64 characters counted as characters, not bytes, are kept.
    let wide = "é".repeat(64);
    assert_eq!(
        session_reset(json!(wide)).resets_text.as_deref(),
        Some(wide.as_str())
    );
}

// ---------------------------------------------------------------------------
// Model entries
// ---------------------------------------------------------------------------

#[test]
fn model_entries_keep_their_order_and_skip_bad_ones() {
    let r = with(|p| {
        limits(p)["model_scoped"] = json!([
            {"display_name": "Opus", "utilization": 40, "resets_at": WEEK_RESET},
            {"utilization": 1, "resets_at": WEEK_RESET},
            {"display_name": format!("Bad{RLO}"), "utilization": 1},
            {"display_name": "x".repeat(65), "utilization": 1},
            {"display_name": "", "utilization": 1},
            {"display_name": 7, "utilization": 1},
            {"display_name": "NoFigure", "utilization": null},
            "Sonnet",
            null,
            {"display_name": "Sonnet", "utilization": 12, "resets_at": 5},
        ]);
    })
    .unwrap();
    let labels: Vec<_> = r.week_models.iter().map(|w| w.label.as_str()).collect();
    assert_eq!(labels, ["Opus", "Sonnet"]);
    assert_eq!(r.week_models[0], window("Opus", 40.0, WEEK_RESET, WEEK_AT));
    assert_eq!(r.week_models[1].resets_text, None);
}

#[test]
fn model_scoped_that_is_not_a_list_gives_no_entries() {
    for v in [
        json!(null),
        json!({"display_name": "Opus", "utilization": 1}),
        json!("x"),
    ] {
        let r = with(|p| limits(p)["model_scoped"] = v.clone()).unwrap();
        assert!(r.week_models.is_empty(), "{v}");
    }
}

// ---------------------------------------------------------------------------
// Issue #1818: no session limit
// ---------------------------------------------------------------------------
//
// `session_limit` is `false` only when `seven_day` gives a window and
// `five_hour` is absent, or has `resets_at` absent or `null`. The check is on
// the raw key, never on the cleaned reset text, so a `resets_at` that is
// present but malformed, empty, too long or unsafe keeps the limit (S1).

/// The #1818 invariants (I1 to I3).
fn assert_invariants(r: &UsageReport) {
    if !r.session_limit {
        assert!(r.session.is_none(), "I1: {r:?}");
        assert!(r.week_all.is_some(), "I1: {r:?}");
        assert!(
            matches!(r.state, UsageState::Ok | UsageState::WeeklyExhausted),
            "I2: {r:?}"
        );
    }
    if r.state == UsageState::SessionExhausted {
        assert!(r.session_limit, "I3: {r:?}");
    }
}

#[test]
fn a_full_answer_has_a_session_limit() {
    let r = parse(&answer(payload())).unwrap();
    assert!(r.session_limit);
    assert_invariants(&r);
}

#[test]
fn a_missing_five_hour_is_no_session_limit() {
    let r = with(|p| {
        limits(p).as_object_mut().unwrap().remove("five_hour");
    })
    .expect("week alone is accepted");
    assert!(!r.session_limit);
    assert_eq!(r.session, None);
    assert_eq!(r.state, UsageState::Ok);
    assert_eq!(
        r.week_all,
        Some(window("all models", 61.0, WEEK_RESET, WEEK_AT))
    );
    assert_invariants(&r);

    let r = with(|p| limits(p)["five_hour"] = json!(null)).expect("week alone");
    assert!(!r.session_limit, "a null five_hour is no window");
    assert_invariants(&r);
}

#[test]
fn a_five_hour_at_0_with_a_null_or_absent_reset_is_no_session_limit() {
    for null in [true, false] {
        let r = with(|p| {
            limits(p)["five_hour"]["utilization"] = json!(0.0);
            if null {
                limits(p)["five_hour"]["resets_at"] = json!(null);
            } else {
                limits(p)["five_hour"]
                    .as_object_mut()
                    .unwrap()
                    .remove("resets_at");
            }
        })
        .unwrap();
        assert!(!r.session_limit, "null={null}");
        assert_eq!(r.session, None, "null={null}");
        assert_eq!(r.state, UsageState::Ok, "null={null}");
        assert_invariants(&r);
    }
}

/// Crusty round 1 on #1829: a non-zero `utilization` is a session window
/// even with no reset, so it keeps the limit and can be exhausted.
#[test]
fn a_five_hour_above_0_with_a_null_or_absent_reset_keeps_the_session_limit() {
    for (pct, want) in [
        (0.5, UsageState::Ok),
        (47.0, UsageState::Ok),
        (100.0, UsageState::SessionExhausted),
        (150.0, UsageState::SessionExhausted),
    ] {
        for null in [true, false] {
            let r = with(|p| {
                limits(p)["five_hour"]["utilization"] = json!(pct);
                if null {
                    limits(p)["five_hour"]["resets_at"] = json!(null);
                } else {
                    limits(p)["five_hour"]
                        .as_object_mut()
                        .unwrap()
                        .remove("resets_at");
                }
            })
            .unwrap();
            assert!(r.session_limit, "{pct} null={null}");
            let s = r.session.as_ref().expect("session kept");
            assert_eq!(s.used_percentage, pct, "{pct} null={null}");
            assert_eq!((s.resets_text.as_deref(), s.resets_at), (None, None));
            assert_eq!(r.state, want, "{pct} null={null}");
            assert_invariants(&r);
        }
    }
}

#[test]
fn no_session_limit_with_a_full_week_is_weekly_exhausted() {
    let r = with(|p| {
        limits(p).as_object_mut().unwrap().remove("five_hour");
        limits(p)["seven_day"]["utilization"] = json!(100.0);
    })
    .unwrap();
    assert_eq!(r.state, UsageState::WeeklyExhausted);
    assert!(!r.session_limit);
    assert_invariants(&r);
}

#[test]
fn a_present_reset_keeps_the_session_limit_and_session_exhaustion() {
    let r = with(|p| limits(p)["five_hour"]["utilization"] = json!(100.0)).unwrap();
    assert_eq!(r.state, UsageState::SessionExhausted);
    assert!(r.session_limit);
    assert_invariants(&r);
}

/// S1: a `resets_at` that is present but cannot be used is still a printed
/// reset, so the login keeps its session limit.
#[test]
fn a_present_but_unusable_reset_keeps_the_session_limit() {
    let long = "x".repeat(65);
    let unsafe_text = format!("2026-10-16T17:00:00Z{RLO}");
    for v in [
        json!(""),
        json!(0),
        json!(1_792_170_000),
        json!("tomorrow at 5pm"),
        json!("2099-01-01T00:00:00Z"),
        json!(long),
        json!(unsafe_text),
        json!({}),
        json!([]),
        json!(false),
    ] {
        let r = with(|p| {
            limits(p)["five_hour"]["utilization"] = json!(100.0);
            limits(p)["five_hour"]["resets_at"] = v.clone();
        })
        .unwrap();
        assert!(r.session_limit, "{v}");
        assert_eq!(r.state, UsageState::SessionExhausted, "{v}");
        assert_invariants(&r);
    }
}

/// S1: a `five_hour` that `window()` rejects, with a reset present, leaves
/// no session window and keeps the session limit.
#[test]
fn a_rejected_five_hour_with_a_reset_keeps_the_session_limit() {
    for v in [json!(null), json!("37"), json!(-1), json!(1_000_001)] {
        let r = with(|p| limits(p)["five_hour"]["utilization"] = v.clone()).unwrap();
        assert_eq!(r.session, None, "{v}");
        assert!(r.session_limit, "{v}");
        assert_eq!(r.state, UsageState::Ok, "{v}");
    }
}

/// S1: a `five_hour` that `window()` rejects keeps the session limit even
/// when its `resets_at` is `null`, because the window could not be read.
#[test]
fn a_rejected_five_hour_with_a_null_reset_keeps_the_session_limit() {
    let r = with(|p| {
        limits(p)["five_hour"]["utilization"] = json!("x");
        limits(p)["five_hour"]["resets_at"] = json!(null);
    })
    .expect("the week is still read");
    assert!(r.session_limit);
    assert_eq!(r.session, None);
    assert_eq!(r.state, UsageState::Ok);
    assert_invariants(&r);
}

#[test]
fn no_week_window_is_still_handled_as_before() {
    // five_hour with no reset and no usable week: nothing to read, so the
    // existing fallback to `/usage` still applies.
    assert_eq!(
        with(|p| {
            limits(p)["five_hour"]["resets_at"] = json!(null);
            limits(p).as_object_mut().unwrap().remove("seven_day");
        })
        .map(|r| (r.session_limit, r.session.is_some())),
        // The session window with no reset and no week is read as before.
        Some((true, true))
    );
    assert_eq!(
        with(|p| {
            limits(p).as_object_mut().unwrap().remove("five_hour");
            limits(p)["seven_day"]["utilization"] = json!(-3);
        }),
        None,
        "neither window: fall back"
    );
}
