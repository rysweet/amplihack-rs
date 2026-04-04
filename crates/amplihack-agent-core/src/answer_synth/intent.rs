//! Intent detection and question classification.
//!
//! Ports `_detect_intent` and enumeration-keyword override logic.

use std::collections::HashMap;

use serde_json::Value;
use tracing::info;

use super::types::{DetectedIntent, ENUMERATION_KEYWORDS, IntentType};
use crate::agentic_loop::traits::LlmClient;
use crate::agentic_loop::types::LlmMessage;
use crate::error::AgentError;

/// Classify the intent of `question` via a single LLM call.
pub async fn detect_intent(
    question: &str,
    llm: &dyn LlmClient,
    model: &str,
) -> Result<DetectedIntent, AgentError> {
    let prompt = build_intent_prompt(question);
    let messages = [
        LlmMessage::system("You are an intent classifier. Respond with ONLY a JSON object."),
        LlmMessage::user(prompt),
    ];
    let raw = llm.completion(&messages, model, 0.0).await?;
    let mut intent = parse_intent_response(&raw);

    let q_lower = question.to_lowercase();
    if ENUMERATION_KEYWORDS.iter().any(|kw| q_lower.contains(kw)) && !intent.intent.is_aggregation()
    {
        info!(
            question = &question[..question.len().min(60)],
            "Enumeration keywords detected; routing to aggregation"
        );
        intent.intent = IntentType::MetaMemory;
    }
    Ok(intent)
}

fn build_intent_prompt(question: &str) -> String {
    format!(
        "Classify the intent of this question into exactly ONE category:\n\n\
         Categories:\n\
         - simple_recall: Direct factual lookup\n\
         - multi_source_synthesis: Combining info from multiple sources\n\
         - temporal_comparison: Timelines, before/after, changes over time\n\
         - mathematical_computation: Arithmetic or numerical computation\n\
         - ratio_trend_analysis: Computing ratios and analyzing trends\n\
         - contradiction_resolution: Resolving conflicting information\n\
         - meta_memory: Questions about what the agent knows, counts of topics\n\
         - incremental_update: Updating existing knowledge with new information\n\
         - causal_counterfactual: \"What if\" hypothetical reasoning\n\n\
         Question: {question}\n\n\
         Respond with JSON: {{\"intent\": \"<category>\", \"needs_temporal\": <bool>, \"needs_math\": <bool>}}"
    )
}

/// Parse the raw LLM JSON response into a [`DetectedIntent`].
pub fn parse_intent_response(raw: &str) -> DetectedIntent {
    let cleaned = strip_markdown_fences(raw);
    let parsed: HashMap<String, Value> = match serde_json::from_str(&cleaned) {
        Ok(m) => m,
        Err(_) => return DetectedIntent::default(),
    };
    let intent = parsed
        .get("intent")
        .and_then(Value::as_str)
        .map(IntentType::parse)
        .unwrap_or_default();
    let needs_temporal = parsed
        .get("needs_temporal")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let needs_math = parsed
        .get("needs_math")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    DetectedIntent {
        intent,
        needs_temporal,
        needs_math,
        ..Default::default()
    }
}

fn strip_markdown_fences(s: &str) -> String {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("```") {
        let rest = rest.split_once('\n').map_or(rest, |(_, after)| after);
        rest.strip_suffix("```").unwrap_or(rest).trim().to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_json() {
        let raw =
            r#"{"intent": "temporal_comparison", "needs_temporal": true, "needs_math": false}"#;
        let i = parse_intent_response(raw);
        assert_eq!(i.intent, IntentType::TemporalComparison);
        assert!(i.needs_temporal);
        assert!(!i.needs_math);
    }

    #[test]
    fn parse_with_markdown_fences() {
        let raw = "```json\n{\"intent\": \"meta_memory\", \"needs_temporal\": false, \"needs_math\": false}\n```";
        assert_eq!(parse_intent_response(raw).intent, IntentType::MetaMemory);
    }

    #[test]
    fn parse_garbage_returns_default() {
        assert_eq!(
            parse_intent_response("not json").intent,
            IntentType::SimpleRecall
        );
    }

    #[test]
    fn parse_missing_fields() {
        let i = parse_intent_response(r#"{"intent": "mathematical_computation"}"#);
        assert_eq!(i.intent, IntentType::MathematicalComputation);
        assert!(!i.needs_temporal && !i.needs_math);
    }

    #[test]
    fn strip_fences() {
        assert_eq!(strip_markdown_fences("hello"), "hello");
        assert_eq!(
            strip_markdown_fences("```json\n{\"a\":1}\n```"),
            "{\"a\":1}"
        );
    }

    #[test]
    fn prompt_contains_question() {
        let p = build_intent_prompt("What is Rust?");
        assert!(p.contains("What is Rust?") && p.contains("simple_recall"));
    }

    #[test]
    fn enumeration_override() {
        let mut i = parse_intent_response(r#"{"intent": "simple_recall"}"#);
        let q = "list all incidents in 2024";
        if ENUMERATION_KEYWORDS
            .iter()
            .any(|kw| q.to_lowercase().contains(kw))
            && !i.intent.is_aggregation()
        {
            i.intent = IntentType::MetaMemory;
        }
        assert_eq!(i.intent, IntentType::MetaMemory);
    }
}
