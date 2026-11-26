//! Pure, total parsing of a Codex `account/rateLimits/read` answer.
//!
//! Every function here returns a value for any input: a field that is
//! missing or of the wrong kind counts as absent, and a `result` that is not
//! an object is no reading at all. Strings from the server are cut to
//! [`MAX_TEXT`] characters and cleaned before they are stored.

use super::{Credits, Reading, Window, WindowName};
use serde_json::{Map, Value};

/// Characters kept from any string the server sends.
pub(super) const MAX_TEXT: usize = 200;

/// Lower-case phrases of an error message that mean "not signed in" (D1).
///
/// Checked against codex-cli 0.166.0 on 2026-11-07: `codex app-server` with
/// an empty `CODEX_HOME` answers `account/rateLimits/read` with error -32600,
/// "codex account authentication required to read rate limits", which the
/// first phrase matches. The test `the_real_not_signed_in_wording_is_not_signed_in`
/// replays that exact answer.
const NOT_SIGNED_IN: [&str; 4] = [
    "authentication required",
    "not logged in",
    "not signed in",
    "login required",
];

/// A finite, non-negative number: a JSON number, or a string of digits with
/// an optional decimal point and more digits. Anything else is `None`.
pub(super) fn num(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) if is_decimal(s) => s.parse::<f64>().ok()?,
        _ => return None,
    };
    (n.is_finite() && n >= 0.0).then_some(n)
}

/// `^[0-9]+(\.[0-9]+)?$`.
fn is_decimal(s: &str) -> bool {
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (s, None),
    };
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    digits(whole) && frac.is_none_or(digits)
}

/// Whether a JSON-RPC error message says the account is not signed in.
pub(super) fn is_not_signed_in(message: &str) -> bool {
    let lower = message.to_lowercase();
    NOT_SIGNED_IN.iter().any(|p| lower.contains(p))
}

/// A server string cut to [`MAX_TEXT`] characters (on a char boundary) with
/// control characters and bidirectional overrides replaced by `?`.
pub(super) fn clean(s: &str) -> String {
    s.chars()
        .take(MAX_TEXT)
        .map(|c| match c {
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => '?',
            c if c.is_control() => '?',
            c => c,
        })
        .collect()
}

/// The figures of an `account/rateLimits/read` `result`: from
/// `result.rateLimits`, `result.rate_limits` or `result` itself, each field
/// in camelCase or snake_case. `None` when `result` is not an object.
pub(super) fn reading(result: &Value) -> Option<Reading> {
    let top = result.as_object()?;
    let nested = ["rateLimits", "rate_limits"]
        .iter()
        .find_map(|k| top.get(*k).and_then(Value::as_object));
    // The nested object first, then the result itself for anything the
    // nested object does not carry.
    let scopes: Vec<&Map<String, Value>> = nested.into_iter().chain([top]).collect();
    let get = |camel: &str, snake: &str| field(&scopes, camel, snake);

    let windows = [
        (WindowName::Primary, "primary"),
        (WindowName::Secondary, "secondary"),
    ]
    .into_iter()
    .filter_map(|(name, key)| {
        get(key, key)
            .and_then(Value::as_object)
            .map(|w| window(name, w))
    })
    .collect();
    let credits = get("credits", "credits")
        .and_then(Value::as_object)
        .map(|c| Credits {
            has_credits: matches!(
                field(&[c], "hasCredits", "has_credits"),
                Some(Value::Bool(true))
            ),
            balance: field(&[c], "balance", "balance").and_then(num),
        });
    Some(Reading {
        windows,
        credits,
        plan_type: get("planType", "plan_type").and_then(text),
        rate_limit_reached_type: get("rateLimitReachedType", "rate_limit_reached_type")
            .and_then(text),
        ordinary_usage_allowed: get("ordinaryUsageAllowed", "ordinary_usage_allowed")
            .and_then(Value::as_bool),
    })
}

fn window(name: WindowName, w: &Map<String, Value>) -> Window {
    let get = |camel: &str, snake: &str| field(&[w], camel, snake);
    Window {
        name,
        used_percent: get("usedPercent", "used_percent").and_then(num),
        window_minutes: get("windowDurationMins", "window_duration_mins").and_then(Value::as_u64),
        resets_at: get("resetsAt", "resets_at").and_then(Value::as_i64),
    }
}

/// The first non-null value of `camel` or `snake` in the first scope that
/// has one.
fn field<'a>(scopes: &[&'a Map<String, Value>], camel: &str, snake: &str) -> Option<&'a Value> {
    scopes.iter().find_map(|m| {
        [camel, snake]
            .iter()
            .find_map(|k| m.get(*k).filter(|v| !v.is_null()))
    })
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(clean)
}

#[cfg(test)]
mod pure {
    use super::*;
    use serde_json::json;

    #[test]
    fn decimal_strings_need_digits_on_both_sides_of_the_point() {
        for ok in ["0", "12", "12.5", "007"] {
            assert!(is_decimal(ok), "{ok}");
        }
        for bad in ["", ".", "12.", ".5", "1.2.3", "+1", "1_000"] {
            assert!(!is_decimal(bad), "{bad}");
        }
    }

    #[test]
    fn clean_keeps_plain_text_and_cuts_by_chars() {
        assert_eq!(clean("plus"), "plus");
        assert_eq!(clean(&"é".repeat(250)).chars().count(), MAX_TEXT);
        assert_eq!(clean("a\u{1b}b\u{2066}c"), "a?b?c");
    }

    #[test]
    fn nested_figures_win_over_the_result_itself() {
        let r = reading(&json!({
            "rateLimits": {"planType": "inner"},
            "planType": "outer",
            "ordinaryUsageAllowed": true
        }))
        .unwrap();
        assert_eq!(r.plan_type.as_deref(), Some("inner"));
        assert_eq!(r.ordinary_usage_allowed, Some(true), "from the result");
    }
}
