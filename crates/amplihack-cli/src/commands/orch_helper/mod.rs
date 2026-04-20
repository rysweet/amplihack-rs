//! Shared orchestration helper functions for smart-orchestrator recipe.
//!
//! Provides `extract-json` and `normalise-type` subcommands used by the
//! parse-decomposition and create-workstreams-config bash steps.

use anyhow::Result;
use std::io::Read;
use std::sync::LazyLock;

/// Maximum stdin read size (10 MB) — prevents OOM from malicious/infinite input.
const MAX_STDIN_BYTES: u64 = 10 * 1024 * 1024;

// Compile regexes once.
static JSON_BLOCK_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?s)```json\s*(\{[^`]*\})\s*```").unwrap());
static UNTAGGED_BLOCK_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?s)```\s*(\{[^`]*\})\s*```").unwrap());

/// Extract the first complete JSON object from LLM output on stdin.
pub fn run_extract_json() -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .take(MAX_STDIN_BYTES)
        .read_to_string(&mut input)?;

    let result = extract_json(&input);
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

/// Normalise LLM task_type on stdin to one of: Q&A, Operations, Investigation, Development.
pub fn run_normalise_type() -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .take(MAX_STDIN_BYTES)
        .read_to_string(&mut input)?;

    println!("{}", normalise_type(input.trim()));
    Ok(())
}

/// Generate workstream config JSON from decomposition JSON on stdin.
pub fn run_generate_workstream_config() -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .take(MAX_STDIN_BYTES)
        .read_to_string(&mut input)?;

    let decomp = extract_json(&input);
    println!("{}", serde_json::to_string(&decomp)?);
    Ok(())
}

fn extract_json(text: &str) -> serde_json::Value {
    // 1. Prefer ```json-tagged code blocks.
    for cap in JSON_BLOCK_RE.captures_iter(text) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cap[1]) {
            return val;
        }
    }

    // 2. Try untagged code blocks.
    for cap in UNTAGGED_BLOCK_RE.captures_iter(text) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cap[1]) {
            return val;
        }
    }

    // 3. Fallback: scan for first valid JSON object using serde_json's StreamDeserializer.
    let mut pos = 0;
    let bytes = text.as_bytes();
    while pos < bytes.len() {
        if let Some(start) = text[pos..].find('{') {
            let abs_start = pos + start;
            let slice = &text[abs_start..];
            let mut de = serde_json::Deserializer::from_str(slice).into_iter::<serde_json::Value>();
            if let Some(Ok(val)) = de.next() {
                return val;
            }
            pos = abs_start + 1;
        } else {
            break;
        }
    }

    serde_json::Value::Object(serde_json::Map::new())
}

fn normalise_type(raw: &str) -> &'static str {
    let t = raw.to_lowercase();
    if ["q&a", "qa", "question", "answer"]
        .iter()
        .any(|k| t.contains(k))
    {
        return "Q&A";
    }
    if ["ops", "operation", "admin", "command"]
        .iter()
        .any(|k| t.contains(k))
    {
        return "Operations";
    }
    if ["invest", "research", "explor", "analys", "understand"]
        .iter()
        .any(|k| t.contains(k))
    {
        return "Investigation";
    }
    "Development"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_tagged_code_block() {
        let input = "Here:\n```json\n{\"task_type\": \"dev\", \"count\": 3}\n```\nDone.";
        let result = extract_json(input);
        assert_eq!(result["task_type"], "dev");
        assert_eq!(result["count"], 3);
    }

    #[test]
    fn extract_json_prefers_tagged_over_untagged() {
        let input = "```\n{\"wrong\": true}\n```\n```json\n{\"right\": true}\n```";
        let result = extract_json(input);
        assert_eq!(result["right"], true);
    }

    #[test]
    fn extract_json_raw_in_prose() {
        let input = r#"The output is {"key": "value"} and more"#;
        let result = extract_json(input);
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn extract_json_no_json_returns_empty_object() {
        let result = extract_json("no json here at all");
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn extract_json_handles_braces_in_strings() {
        let input = r#"Result: {"msg": "brace } inside", "ok": true} done"#;
        let result = extract_json(input);
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn extract_json_deeply_nested() {
        let input = r#"{"a": {"b": {"c": [1, 2, {"d": "deep"}]}}}"#;
        let result = extract_json(input);
        assert_eq!(result["a"]["b"]["c"][2]["d"], "deep");
    }

    #[test]
    fn normalise_type_qa() {
        assert_eq!(normalise_type("Q&A"), "Q&A");
        assert_eq!(normalise_type("qa"), "Q&A");
        assert_eq!(normalise_type("question about code"), "Q&A");
    }

    #[test]
    fn normalise_type_operations() {
        assert_eq!(normalise_type("ops"), "Operations");
        assert_eq!(normalise_type("Operations"), "Operations");
        assert_eq!(normalise_type("admin task"), "Operations");
    }

    #[test]
    fn normalise_type_investigation() {
        assert_eq!(normalise_type("investigation"), "Investigation");
        assert_eq!(normalise_type("research task"), "Investigation");
        assert_eq!(normalise_type("explore the code"), "Investigation");
    }

    #[test]
    fn normalise_type_development_default() {
        assert_eq!(normalise_type("dev"), "Development");
        assert_eq!(normalise_type("build feature"), "Development");
        assert_eq!(normalise_type("anything else"), "Development");
        assert_eq!(normalise_type(""), "Development");
    }

    #[test]
    fn normalise_type_case_insensitive() {
        assert_eq!(normalise_type("Q&A"), "Q&A");
        assert_eq!(normalise_type("q&a"), "Q&A");
        assert_eq!(normalise_type("OPS"), "Operations");
        assert_eq!(normalise_type("INVESTIGATION"), "Investigation");
    }
}
