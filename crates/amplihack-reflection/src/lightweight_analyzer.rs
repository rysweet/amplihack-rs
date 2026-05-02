//! Time-budgeted heuristic analyzer (port of `lightweight_analyzer.py`).
//!
//! The Python implementation called the Claude SDK; the Rust port keeps the
//! same response shape but uses purely deterministic keyword heuristics so
//! it can be unit-tested without external services.

use std::time::Instant;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatternKind {
    Error,
    Inefficiency,
    AutomationOpportunity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    #[serde(rename = "type")]
    pub kind: PatternKind,
    pub description: String,
    pub severity: Severity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub patterns: Vec<Pattern>,
    pub summary: String,
    pub elapsed_seconds: f64,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct LightweightAnalyzer {
    pub max_seconds: f64,
}

impl LightweightAnalyzer {
    pub fn new() -> Self {
        Self { max_seconds: 5.0 }
    }

    pub fn analyze_recent_responses(
        &self,
        messages: &[Message],
        tool_logs: &[String],
    ) -> anyhow::Result<AnalysisResult> {
        let start = Instant::now();
        let assistant: Vec<&Message> = messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .collect();
        if assistant.is_empty() {
            return Ok(AnalysisResult {
                patterns: vec![],
                summary: "Not enough messages to analyze".to_string(),
                elapsed_seconds: start.elapsed().as_secs_f64(),
            });
        }

        let recent: Vec<&&Message> = assistant.iter().rev().take(2).collect();
        let mut blob = String::new();
        for m in recent {
            blob.push_str(&m.content);
            blob.push('\n');
        }
        for log in tool_logs.iter().rev().take(10) {
            blob.push_str(log);
            blob.push('\n');
        }
        let lower = blob.to_ascii_lowercase();

        let mut patterns = Vec::new();
        if lower.contains("error") || lower.contains("failed") {
            patterns.push(Pattern {
                kind: PatternKind::Error,
                description: "Error detected in recent interaction".to_string(),
                severity: Severity::High,
            });
        }
        if lower.contains("timeout") || lower.contains("timed out") {
            patterns.push(Pattern {
                kind: PatternKind::Inefficiency,
                description: "Timeout detected, may indicate performance issue".to_string(),
                severity: Severity::Medium,
            });
        }
        if lower.matches("tried").count() >= 2 {
            patterns.push(Pattern {
                kind: PatternKind::AutomationOpportunity,
                description: "Repeated retries detected; consider automation".to_string(),
                severity: Severity::Low,
            });
        }

        let elapsed = start.elapsed().as_secs_f64();
        let n = patterns.len();
        Ok(AnalysisResult {
            patterns,
            summary: format!("Found {n} patterns in {elapsed:.1}s"),
            elapsed_seconds: elapsed,
        })
    }
}
