//! Helpers used by [`super::loop_core`].

use std::collections::HashMap;

use serde_json::Value;

/// Build an "error" action decision when reasoning fails.
pub(crate) fn error_action(reasoning: &str, error_msg: &str) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    map.insert("reasoning".into(), Value::String(reasoning.into()));
    map.insert("action".into(), Value::String("error".into()));
    let mut params = serde_json::Map::new();
    params.insert("error".into(), Value::String(error_msg.into()));
    map.insert("params".into(), Value::Object(params));
    map
}

/// Truncate a string to at most `max_len` bytes without splitting a
/// multi-byte character.
pub(crate) fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_basic() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello");
    }

    #[test]
    fn truncate_multibyte() {
        let s = "héllo";
        let t = truncate(s, 2);
        assert!(t.len() <= 2);
        assert!(t.is_ascii() || !t.is_empty());
    }

    #[test]
    fn error_action_helper() {
        let m = error_action("reason", "err");
        assert_eq!(m["action"], Value::String("error".into()));
        assert_eq!(m["reasoning"], Value::String("reason".into()));
    }
}
