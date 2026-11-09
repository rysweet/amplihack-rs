//! Whether a Codex session stopped on a usage limit. The wording Codex
//! uses may change between versions, so the whole match lives here and is
//! covered by the fixtures under `testdata/`.

use amplihack_utils::codex_capacity::Reading;

/// The usage-limit text, matched ignoring ASCII case, with a straight
/// apostrophe; [`CURLY`] is the same with a curly one.
const TEXT: &str = "you've hit your usage limit";
const CURLY: &str = "you\u{2019}ve hit your usage limit";
/// The usage-limit error code.
const CODE: &str = "usage_limit_reached";
/// How deep an event is searched.
const MAX_DEPTH: usize = 32;

/// A live reading is limited when Codex reports which limit is reached.
pub(super) fn limit_hit_reading(r: &Reading) -> bool {
    r.rate_limit_reached_type.is_some()
}

/// Whether `text` holds the usage-limit wording or code.
pub(super) fn limit_hit_text(text: &str) -> bool {
    if text.contains(CODE) {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    lower.contains(TEXT) || lower.contains(CURLY)
}

/// Whether one event shows a usage limit: a non-null
/// `rate_limit_reached_type` (or `rateLimitReachedType`) at any depth, or a
/// string value holding the usage-limit wording or code.
pub(super) fn limit_hit_event(v: &serde_json::Value) -> bool {
    hit(v, 0)
}

fn hit(v: &serde_json::Value, depth: usize) -> bool {
    use serde_json::Value;
    if depth > MAX_DEPTH {
        return false;
    }
    match v {
        Value::String(s) => limit_hit_text(s),
        Value::Array(a) => a.iter().any(|x| hit(x, depth + 1)),
        Value::Object(m) => m.iter().any(|(k, x)| {
            let reached = matches!(
                k.as_str(),
                "rate_limit_reached_type" | "rateLimitReachedType"
            ) && !x.is_null();
            reached || hit(x, depth + 1)
        }),
        _ => false,
    }
}
