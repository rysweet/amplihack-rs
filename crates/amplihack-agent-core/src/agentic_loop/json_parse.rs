//! JSON parsing helpers for LLM responses.
//!
//! LLMs often wrap JSON in markdown code fences. This module extracts and
//! parses JSON from raw response text, mirroring the Python
//! `_parse_json_response` static method.

use std::collections::HashMap;

use serde_json::Value;

/// Parse a JSON object from an LLM response, handling markdown code blocks.
///
/// Tries (in order):
/// 1. Direct `serde_json::from_str`
/// 2. Extract from `` ```json … ``` ``
/// 3. Extract from `` ``` … ``` ``
///
/// Returns `None` if parsing fails or the result is not a JSON object.
pub fn parse_json_response(response_text: &str) -> Option<HashMap<String, Value>> {
    let text = response_text.trim();
    if text.is_empty() {
        return None;
    }

    // 1. Try direct parse.
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) {
        return Some(map.into_iter().collect());
    }

    // 2. Try ` ```json … ``` `.
    if let Some(start) = text.find("```json") {
        let inner_start = start + 7;
        if let Some(end) = text[inner_start..].find("```") {
            let slice = text[inner_start..inner_start + end].trim();
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(slice) {
                return Some(map.into_iter().collect());
            }
        }
    }

    // 3. Try generic ` ``` … ``` `.
    if let Some(start) = text.find("```") {
        let inner_start = start + 3;
        // Skip optional language tag on the same line.
        let inner_start = text[inner_start..]
            .find('\n')
            .map_or(inner_start, |nl| inner_start + nl + 1);
        if let Some(end) = text[inner_start..].find("```") {
            let slice = text[inner_start..inner_start + end].trim();
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(slice) {
                return Some(map.into_iter().collect());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_json() {
        let input = r#"{"action": "greet", "params": {}}"#;
        let result = parse_json_response(input).unwrap();
        assert_eq!(result["action"], Value::String("greet".into()));
    }

    #[test]
    fn json_in_code_fence() {
        let input = "```json\n{\"key\": \"value\"}\n```";
        let result = parse_json_response(input).unwrap();
        assert_eq!(result["key"], Value::String("value".into()));
    }

    #[test]
    fn json_in_generic_fence() {
        let input = "```\n{\"a\": 1}\n```";
        let result = parse_json_response(input).unwrap();
        assert_eq!(result["a"], Value::Number(1.into()));
    }

    #[test]
    fn returns_none_on_empty() {
        assert!(parse_json_response("").is_none());
        assert!(parse_json_response("   ").is_none());
    }

    #[test]
    fn returns_none_on_non_object() {
        assert!(parse_json_response("[1,2,3]").is_none());
        assert!(parse_json_response("\"hello\"").is_none());
    }

    #[test]
    fn returns_none_on_garbage() {
        assert!(parse_json_response("this is not json at all").is_none());
    }

    #[test]
    fn json_with_surrounding_text() {
        let input = "Here is the answer:\n```json\n{\"ok\": true}\n```\nDone.";
        let result = parse_json_response(input).unwrap();
        assert_eq!(result["ok"], Value::Bool(true));
    }

    #[test]
    fn whitespace_padded_json() {
        let input = "  \n  {\"x\": 42}  \n  ";
        let result = parse_json_response(input).unwrap();
        assert_eq!(result["x"], Value::Number(42.into()));
    }
}
