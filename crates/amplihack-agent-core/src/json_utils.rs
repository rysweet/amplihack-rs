//! Shared JSON parsing utilities for LLM responses.
//!
//! Ports Python `amplihack/agents/goal_seeking/json_utils.py`:
//! - Direct JSON parse
//! - Extract from ```json ... ``` blocks
//! - Find first { ... } block
//! - JSON list extraction

use regex::Regex;

/// Parse JSON from an LLM response, handling markdown code blocks.
///
/// Tries multiple extraction strategies:
/// 1. Direct JSON parse
/// 2. Extract from ```json ... ``` blocks
/// 3. Find first { ... } block
pub fn parse_llm_json(response_text: &str) -> Option<serde_json::Value> {
    if response_text.is_empty() {
        return None;
    }
    let text = response_text.trim();

    // Strategy 1: Direct parse
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(text)
        && val.is_object()
    {
        return Some(val);
    }

    // Strategy 2: Extract from fenced code blocks
    let fenced_re = Regex::new(r"(?s)```(?:json)?\s*\n?(.*?)\n?\s*```").ok()?;
    if let Some(caps) = fenced_re.captures(text) {
        let inner = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(inner)
            && val.is_object()
        {
            return Some(val);
        }
    }

    // Strategy 3: Find first { ... } block
    let brace_re = Regex::new(r"(?s)\{.*\}").ok()?;
    if let Some(m) = brace_re.find(text)
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(m.as_str())
        && val.is_object()
    {
        return Some(val);
    }

    None
}

/// Parse a JSON list from an LLM response.
///
/// Returns empty vec if parsing fails.
pub fn parse_llm_json_list(response_text: &str) -> Vec<serde_json::Value> {
    if response_text.is_empty() {
        return Vec::new();
    }
    let text = response_text.trim();

    // Direct parse
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(text)
        && let Some(arr) = val.as_array()
    {
        return arr.clone();
    }

    // Extract from code block
    if let Ok(re) = Regex::new(r"(?s)```(?:json)?\s*\n?(.*?)\n?\s*```")
        && let Some(caps) = re.captures(text)
    {
        let inner = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(inner)
            && let Some(arr) = val.as_array()
        {
            return arr.clone();
        }
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_direct_json() {
        let input = r#"{"key": "value"}"#;
        let result = parse_llm_json(input).unwrap();
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn parse_fenced_json() {
        let input = "Here is the result:\n```json\n{\"key\": \"val\"}\n```";
        let result = parse_llm_json(input).unwrap();
        assert_eq!(result["key"], "val");
    }

    #[test]
    fn parse_embedded_braces() {
        let input = "Some text before {\"answer\": 42} and after";
        let result = parse_llm_json(input).unwrap();
        assert_eq!(result["answer"], 42);
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse_llm_json("").is_none());
        assert!(parse_llm_json("no json here").is_none());
    }

    #[test]
    fn parse_list_direct() {
        let input = r#"[{"a": 1}, {"b": 2}]"#;
        let result = parse_llm_json_list(input);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn parse_list_fenced() {
        let input = "```json\n[{\"x\": 1}]\n```";
        let result = parse_llm_json_list(input);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn parse_list_empty() {
        assert!(parse_llm_json_list("").is_empty());
        assert!(parse_llm_json_list("not json").is_empty());
    }

    #[test]
    fn parse_json_non_object_skipped() {
        let input = r#""just a string""#;
        assert!(parse_llm_json(input).is_none());
    }
}
