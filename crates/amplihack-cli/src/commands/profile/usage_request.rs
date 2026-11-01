//! The `get_usage` control request (issue #1698): the arguments and the one
//! stdin line that ask `claude` for its usage figures without the transcript
//! scan that makes `/usage` slow, and the reader of the answer. The reader is
//! pure and total: no input makes it panic, and a value it cannot read is
//! left out. Nothing here runs a process, does I/O or names a login file.

use super::usage_cmd::sanitize_display;
use super::usage_parse::{MAX_PCT, UsageReport, UsageState, UsageWindow};
use serde_json::{Map, Value};

/// Fixed: each process gets one request, and a constant line can be quoted
/// byte for byte in the docs and tests.
pub(super) const REQUEST_ID: &str = "amplihack-usage";
/// The one stdin line. It is written with its `\n` in one `write` into an
/// empty pipe; below PIPE_BUF (at least 512 bytes on every POSIX system)
/// that write is atomic and cannot block.
pub(super) const REQUEST_LINE: &str = r#"{"type":"control_request","request_id":"amplihack-usage","request":{"subtype":"get_usage","skip_behaviors":true}}"#;
const _: () = assert!(REQUEST_LINE.len() < 512);
pub(super) const ARGS: [&str; 6] = [
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
];
/// Text from the answer longer than this, in characters, is not used.
const MAX_TEXT: usize = 64;
/// A reset further ahead than this has no epoch: no plan window is longer
/// than a week.
const MAX_AHEAD: i64 = 8 * 86_400;

/// The report in the first `control_response` for [`REQUEST_ID`] in
/// `stdout`, when it is accepted; `None` means fall back to `/usage`. Only
/// lines that mention `control_response` are parsed, and the first matching
/// line decides, even when it is rejected.
pub(super) fn parse_answer(stdout: &str, now: i64) -> Option<UsageReport> {
    let answer = stdout
        .lines()
        .filter(|l| l.contains("control_response"))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| {
            v.get("type").and_then(Value::as_str) == Some("control_response")
                && v.get("response")
                    .and_then(|r| r.get("request_id"))
                    .and_then(Value::as_str)
                    == Some(REQUEST_ID)
        })?;
    let response = answer.get("response")?;
    if response.get("subtype").and_then(Value::as_str) != Some("success") {
        return None;
    }
    let payload = response.get("response")?.as_object()?;
    if payload.get("rate_limits_available") != Some(&Value::Bool(true)) {
        return None;
    }
    let limits = payload.get("rate_limits")?.as_object()?;
    // Credits on: only `/usage` reports the spend limit, so fall back.
    if !credits_off(payload) || !credits_off(limits) {
        return None;
    }
    let session = limits
        .get("five_hour")
        .and_then(|w| window(w, "session", now));
    let week_all = limits
        .get("seven_day")
        .and_then(|w| window(w, "all models", now));
    if session.is_none() && week_all.is_none() {
        return None;
    }
    let week_models = limits
        .get("model_scoped")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |models| {
            models.iter().filter_map(|m| model(m, now)).collect()
        });
    let exhausted =
        |w: &Option<UsageWindow>| w.as_ref().is_some_and(|w| w.used_percentage >= 100.0);
    let state = if exhausted(&session) {
        UsageState::SessionExhausted
    } else if exhausted(&week_all) {
        UsageState::WeeklyExhausted
    } else {
        UsageState::Ok
    };
    Some(UsageReport {
        state,
        session,
        week_all,
        week_models,
    })
}

/// `extra_usage` in `object` is absent, `null`, or an object whose
/// `is_enabled` is the JSON `false`. Anything else counts as credits on.
fn credits_off(object: &Map<String, Value>) -> bool {
    match object.get("extra_usage") {
        None | Some(Value::Null) => true,
        Some(Value::Object(o)) => o.get("is_enabled") == Some(&Value::Bool(false)),
        Some(_) => false,
    }
}

/// One `{utilization, resets_at}` window. `None` unless the utilization is a
/// finite number from 0 to [`MAX_PCT`]; a reset that is not clean text
/// leaves both reset fields out.
fn window(v: &Value, label: &str, now: i64) -> Option<UsageWindow> {
    let pct = v.get("utilization")?.as_f64()?;
    if !(pct.is_finite() && (0.0..=MAX_PCT).contains(&pct)) {
        return None;
    }
    let resets_text = v
        .get("resets_at")
        .and_then(Value::as_str)
        .and_then(clean_text);
    let resets_at = resets_text
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.timestamp())
        .filter(|t| *t <= now.saturating_add(MAX_AHEAD));
    Some(UsageWindow {
        label: label.to_string(),
        // `-0.0 + 0.0` is `0.0`, so the table never shows `-0`.
        used_percentage: pct + 0.0,
        resets_text,
        resets_at,
    })
}

/// One `model_scoped` entry, labelled with its `display_name`.
fn model(v: &Value, now: i64) -> Option<UsageWindow> {
    let label = clean_text(v.get("display_name")?.as_str()?)?;
    window(v, &label, now)
}

/// `s` when it is 1 to 64 characters and the table would show it unchanged:
/// no control character and nothing in U+202A to U+202E or U+2066 to U+2069.
fn clean_text(s: &str) -> Option<String> {
    let chars = s.chars().count();
    ((1..=MAX_TEXT).contains(&chars) && sanitize_display(s, false) == s).then(|| s.to_string())
}
