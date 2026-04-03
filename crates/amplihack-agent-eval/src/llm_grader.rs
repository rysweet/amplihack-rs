//! LLM-based grader — trait and utilities for LLM semantic grading.
//!
//! Matches Python `amplihack/eval/llm_grader.py`:
//! - JSON extraction from LLM responses
//! - Model resolution from config/env
//! - Grading callback abstraction

use crate::error::EvalError;
use crate::models::GradeResult;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Default grader model if none configured.
pub const DEFAULT_GRADER_MODEL: &str = "claude-sonnet-4-20250514";

/// Trait for LLM-backed grading.
pub trait LlmGrader {
    /// Call the LLM and return a parsed grade result.
    fn grade_with_llm(
        &self,
        prompt: &str,
        model: &str,
    ) -> Result<LlmGradeResponse, EvalError>;
}

/// Structured response from an LLM grading call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGradeResponse {
    pub score: f64,
    pub reasoning: String,
    #[serde(default)]
    pub confidence: f64,
}

impl LlmGradeResponse {
    pub fn into_grade_result(self) -> Result<GradeResult, crate::error::EvalError> {
        GradeResult::new(self.score, self.reasoning)
    }
}

/// Get the grader model from explicit config, env, or default.
pub fn get_grader_model(explicit: Option<&str>) -> String {
    if let Some(model) = explicit {
        return model.to_string();
    }
    std::env::var("GRADER_MODEL").unwrap_or_else(|_| DEFAULT_GRADER_MODEL.to_string())
}

/// Extract JSON from LLM text that may contain markdown fences or extra text.
pub fn extract_json(text: &str) -> Option<String> {
    let trimmed = text.trim();

    // Try direct parse
    if ((trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']')))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        return Some(trimmed.to_string());
    }

    // Try extracting from markdown code fences
    if let Some(json) = extract_from_code_fence(trimmed) {
        return Some(json);
    }

    // Try finding first { ... } block
    if let Some(json) = extract_brace_block(trimmed) {
        return Some(json);
    }

    None
}

/// Build a grading prompt for semantic comparison.
pub fn build_grading_prompt(
    question: &str,
    expected: &str,
    actual: &str,
    level: u8,
) -> String {
    let level_guidance = match level {
        3 => "\nIMPORTANT: For L3 temporal reasoning, pay special attention to \
              time-based ordering, recency, and chronological accuracy.",
        5 => "\nIMPORTANT: For L5 contradiction handling, verify the answer \
              correctly identifies and resolves contradicting information.",
        _ => "",
    };

    format!(
        r#"Grade the following answer on a scale of 0.0 to 1.0.
{level_guidance}
Question: {question}

Expected answer: {expected}

Actual answer: {actual}

Respond with JSON: {{"score": <float>, "reasoning": "<explanation>"}}"#
    )
}

/// Stub LLM grader for testing — uses simple text matching.
pub struct StubLlmGrader;

impl LlmGrader for StubLlmGrader {
    fn grade_with_llm(
        &self,
        _prompt: &str,
        _model: &str,
    ) -> Result<LlmGradeResponse, EvalError> {
        debug!("StubLlmGrader: returning default response");
        Ok(LlmGradeResponse {
            score: 0.5,
            reasoning: "Stub grader — no LLM available".into(),
            confidence: 0.0,
        })
    }
}

fn extract_from_code_fence(text: &str) -> Option<String> {
    let markers = ["```json", "```JSON", "```"];
    for marker in markers {
        if let Some(start) = text.find(marker) {
            let content_start = start + marker.len();
            if let Some(end) = text[content_start..].find("```") {
                let json = text[content_start..content_start + end].trim();
                if serde_json::from_str::<serde_json::Value>(json).is_ok() {
                    return Some(json.to_string());
                }
            }
        }
    }
    None
}

fn extract_brace_block(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0;
    for (i, c) in text[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let candidate = &text[start..start + i + 1];
                    if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                        return Some(candidate.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_plain() {
        let text = r#"{"score": 0.8, "reasoning": "good"}"#;
        let json = extract_json(text).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["score"], 0.8);
    }

    #[test]
    fn extract_json_from_code_fence() {
        let text = "Here's the result:\n```json\n{\"score\": 0.9}\n```\nDone.";
        let json = extract_json(text).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["score"], 0.9);
    }

    #[test]
    fn extract_json_embedded() {
        let text = "The answer is {\"score\": 0.7, \"reasoning\": \"ok\"} here";
        let json = extract_json(text).unwrap();
        assert!(json.contains("0.7"));
    }

    #[test]
    fn extract_json_none_for_garbage() {
        assert!(extract_json("no json here").is_none());
        assert!(extract_json("").is_none());
    }

    #[test]
    fn extract_json_array() {
        let text = "[1, 2, 3]";
        assert!(extract_json(text).is_some());
    }

    #[test]
    fn get_grader_model_explicit() {
        assert_eq!(get_grader_model(Some("gpt-4")), "gpt-4");
    }

    #[test]
    fn get_grader_model_default() {
        // When no env var set and no explicit model, returns default
        let model = get_grader_model(None);
        // May be DEFAULT_GRADER_MODEL or env-provided; just check non-empty
        assert!(!model.is_empty());
    }

    #[test]
    fn build_grading_prompt_standard() {
        let prompt = build_grading_prompt("What is 2+2?", "4", "four", 1);
        assert!(prompt.contains("What is 2+2?"));
        assert!(prompt.contains("four"));
        assert!(!prompt.contains("temporal"));
    }

    #[test]
    fn build_grading_prompt_l3() {
        let prompt = build_grading_prompt("When?", "yesterday", "today", 3);
        assert!(prompt.contains("temporal"));
    }

    #[test]
    fn build_grading_prompt_l5() {
        let prompt = build_grading_prompt("Resolve?", "A", "B", 5);
        assert!(prompt.contains("contradiction"));
    }

    #[test]
    fn stub_grader_returns_default() {
        let grader = StubLlmGrader;
        let response = grader.grade_with_llm("test", "model").unwrap();
        assert_eq!(response.score, 0.5);
        assert_eq!(response.confidence, 0.0);
    }

    #[test]
    fn llm_grade_response_to_grade_result() {
        let response = LlmGradeResponse {
            score: 0.85,
            reasoning: "Excellent".into(),
            confidence: 0.9,
        };
        let grade = response.into_grade_result().unwrap();
        assert_eq!(grade.score, 0.85);
        assert_eq!(grade.reasoning, "Excellent");
    }

    #[test]
    fn llm_grade_response_serde() {
        let response = LlmGradeResponse {
            score: 0.7,
            reasoning: "OK".into(),
            confidence: 0.8,
        };
        let json = serde_json::to_string(&response).unwrap();
        let restored: LlmGradeResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.score, 0.7);
    }
}
