//! The `get_usage` control request (issue #1698): the arguments and the one
//! stdin line that ask `claude` for its usage figures without the transcript
//! scan that makes `/usage` slow, and the reader of the answer. The reader is
//! pure and total: no input makes it panic, and a value it cannot read is
//! left out. Nothing here runs a process, does I/O or names a login file.

use super::usage_cmd::sanitize_display;
use super::usage_parse::{MAX_PCT, UsageReport, UsageState, UsageWindow, settle_session};
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
    if !credits_off(payload) {
        return None;
    }
    match parse_limits(payload.get("rate_limits")?, now) {
        Limits::Report {
            report,
            credits_on: None,
        } => Some(report),
        Limits::Report { .. } | Limits::CreditsOn | Limits::Unparseable => None,
    }
}

/// What one `rate_limits`-shaped object holds (issues #1844 and #1856).
// One value per read, returned and taken apart at once: boxing the large
// variant would add an allocation to every read and save nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Limits {
    /// The windows, read as `profile usage` reports them. `credits_on` is
    /// the redacted shape of `extra_usage` when usage credits are on, and
    /// `None` when they are off.
    Report {
        report: UsageReport,
        credits_on: Option<ExtraUsageShape>,
    },
    /// `extra_usage` is on and neither window could be read.
    CreditsOn,
    /// Not an object, or credits are off and neither window could be read.
    Unparseable,
}

/// Reads one `rate_limits`-shaped object: `five_hour`, `seven_day`,
/// `model_scoped` and `extra_usage`. The `get_usage` answer and the direct
/// read (issue #1844) both use it, so issue #1818's rules are the same on
/// both. Unknown keys are ignored.
pub(super) fn parse_limits(limits: &Value, now: i64) -> Limits {
    let Some(limits) = limits.as_object() else {
        return Limits::Unparseable;
    };
    let credits_on = (!credits_off(limits))
        .then(|| ExtraUsageShape::of(limits.get("extra_usage").unwrap_or(&Value::Null)));
    let five_hour = limits.get("five_hour");
    let mut session = five_hour.and_then(|w| window(w, "session", now));
    let week_all = limits
        .get("seven_day")
        .and_then(|w| window(w, "all models", now));
    if session.is_none() && week_all.is_none() {
        return if credits_on.is_some() {
            Limits::CreditsOn
        } else {
            Limits::Unparseable
        };
    }
    let week_models = limits
        .get("model_scoped")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |models| {
            models.iter().filter_map(|m| model(m, now)).collect()
        });
    let exhausted =
        |w: &Option<UsageWindow>| w.as_ref().is_some_and(|w| w.used_percentage >= 100.0);
    // Issue #1818: decided on the raw `five_hour`, never the cleaned text.
    let reset_seen = session_reset_seen(five_hour, session.is_some());
    let mut observation = super::usage_parse::measured_evidence(
        &session,
        &week_all,
        match five_hour {
            Some(Value::Null) => amplihack_utils::usage_observation::SessionWindow::Absent,
            _ if session.is_some() => amplihack_utils::usage_observation::SessionWindow::Present,
            _ => amplihack_utils::usage_observation::SessionWindow::Unknown,
        },
    );
    let raw_resets = observation_reset(five_hour).zip(observation_reset(limits.get("seven_day")));
    if let Some((session_reset, week_reset)) = raw_resets {
        observation.session_reset_at = session_reset;
        observation.week_reset_at = week_reset;
    } else {
        observation = Default::default();
    }
    let session_limit = settle_session(&mut session, reset_seen, week_all.is_some());
    let state = if exhausted(&session) {
        UsageState::SessionExhausted
    } else if exhausted(&week_all) {
        UsageState::WeeklyExhausted
    } else {
        UsageState::Ok
    };
    observation.limit_reached = raw_resets.is_some()
        && matches!(
            state,
            UsageState::SessionExhausted | UsageState::WeeklyExhausted
        );
    Limits::Report {
        report: UsageReport {
            observation,
            state,
            session,
            week_all,
            week_models,
            session_limit,
            observed_at: None,
        },
        credits_on,
    }
}

/// Raw observation resets have no display horizon. Malformed values are
/// refused; absence and JSON null explicitly represent an unknown reset.
fn observation_reset(window: Option<&Value>) -> Option<Option<i64>> {
    match window.and_then(|w| w.get("resets_at")) {
        None | Some(Value::Null) => Some(None),
        Some(value) => value
            .as_i64()
            .or_else(|| {
                value
                    .as_str()
                    .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
                    .map(|time| time.timestamp())
            })
            .map(Some),
    }
}

/// Whether `five_hour` shows a session reset (issue #1818). Only an absent
/// or `null` `five_hour`, or a read window whose `resets_at` is absent or
/// `null`, shows none. A `resets_at` of any other value counts, even when it
/// cannot be used, and so does a `five_hour` the reader rejected (S1). A read
/// window with a non-zero `utilization` keeps the limit even with no reset:
/// [`settle_session`] checks the percentage.
fn session_reset_seen(five_hour: Option<&Value>, read: bool) -> bool {
    match five_hour {
        None | Some(Value::Null) => false,
        Some(w) if read => !matches!(w.get("resets_at"), None | Some(Value::Null)),
        Some(_) => true,
    }
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

/// At most this many `extra_usage` keys are listed in a shape line.
const SHAPE_MAX_KEYS: usize = 32;
/// A listed key longer than this, in characters, is cut.
const SHAPE_MAX_KEY: usize = 64;
/// The `is_enabled` strings a shape line names (issue #1856, D2). The
/// constant is printed, never the bytes from the reply.
const ENABLED_WORDS: [&str; 10] = [
    "true", "false", "on", "off", "yes", "no", "enabled", "disabled", "0", "1",
];

/// The redacted shape of a credits-on `extra_usage` (issue #1856): its keys
/// and the JSON type of each member, with the value only of bools, `null`,
/// and an `is_enabled` that matches [`ENABLED_WORDS`] or is the number zero
/// or one. No other string or number from the reply is kept. It is built
/// only by [`ExtraUsageShape::of`] and is never serialised.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExtraUsageShape {
    line: String,
}

impl ExtraUsageShape {
    /// The shape of `v`. Total: no input fails or panics, and nested values
    /// are named by type only, never walked.
    pub(super) fn of(v: &Value) -> Self {
        let line = match v {
            Value::Object(o) => object_shape(o),
            other => format!("extra_usage: {}", scalar_form(other)),
        };
        Self { line }
    }

    /// The text after `note: profile "<name>": `. Printable ASCII and `…`
    /// only.
    pub(super) fn line(&self) -> &str {
        &self.line
    }
}

/// `extra_usage: object; keys: …; is_enabled: …` and the listed bool and
/// `null` members, in byte order of their keys.
fn object_shape(o: &Map<String, Value>) -> String {
    let mut keys: Vec<&String> = o.keys().collect();
    keys.sort_unstable();
    let listed = &keys[..keys.len().min(SHAPE_MAX_KEYS)];
    let mut line = String::from("extra_usage: object; keys: ");
    if listed.is_empty() {
        line.push_str("none");
    } else {
        let names: Vec<String> = listed.iter().map(|k| shape_key(k)).collect();
        line.push_str(&names.join(", "));
    }
    if keys.len() > listed.len() {
        line.push_str(&format!(" (+{} more)", keys.len() - listed.len()));
    }
    line.push_str("; is_enabled: ");
    line.push_str(&enabled_form(o.get("is_enabled")));
    for key in listed.iter().filter(|k| k.as_str() != "is_enabled") {
        match o.get(key.as_str()) {
            Some(Value::Bool(b)) => line.push_str(&format!("; {}: bool {b}", shape_key(key))),
            Some(Value::Null) => line.push_str(&format!("; {}: null", shape_key(key))),
            _ => {}
        }
    }
    line
}

/// A value other than an object: its type, and a bool's value.
fn scalar_form(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => format!("bool {b}"),
        Value::Number(_) => "number".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(_) => "array".to_string(),
        Value::Object(_) => "object".to_string(),
    }
}

/// The form of `is_enabled` (D2).
fn enabled_form(v: Option<&Value>) -> String {
    match v {
        None => "missing".to_string(),
        Some(Value::String(s)) => match ENABLED_WORDS.iter().find(|w| w.eq_ignore_ascii_case(s)) {
            Some(word) => format!("string \"{word}\""),
            None => format!("string ({} chars)", s.chars().count()),
        },
        // An integer or a float exactly equal to 0 or 1 is the flag (S3);
        // no tolerance, so 0.9999 stays a bare `number`.
        Some(Value::Number(n)) => match n.as_u64().or_else(|| match n.as_f64() {
            Some(0.0) => Some(0),
            Some(1.0) => Some(1),
            _ => None,
        }) {
            Some(0) => "number 0".to_string(),
            Some(1) => "number 1".to_string(),
            _ => "number".to_string(),
        },
        Some(other) => scalar_form(other),
    }
}

/// A key as a shape line shows it (S1, S2): every character outside
/// `[A-Za-z0-9_.-]` becomes `?`, so no key can forge a separator; a key
/// longer than 64 characters is cut to 63 and `…`; an empty key is `""`.
fn shape_key(key: &str) -> String {
    if key.is_empty() {
        return "\"\"".to_string();
    }
    let safe = key.chars().map(|c| {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            c
        } else {
            '?'
        }
    });
    if key.chars().count() > SHAPE_MAX_KEY {
        let mut cut: String = safe.take(SHAPE_MAX_KEY - 1).collect();
        cut.push('…');
        cut
    } else {
        safe.collect()
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
