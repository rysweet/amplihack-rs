//! Contextual error analyzer (port of `contextual_error_analyzer.py`).
//!
//! Deterministic keyword-based analysis. The Python version had an optional
//! Claude SDK path; we keep only the keyword fallback because it must be
//! reproducible and run offline.

use serde::{Deserialize, Serialize};

use crate::security::{filter_pattern_suggestion, sanitize_content};

use super::patterns;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    ImportError,
    Permission,
    Network,
    FileMissing,
    Syntax,
    Type,
    Index,
    Key,
    Value,
    CommandMissing,
    Memory,
    Generic,
    Unknown,
    NoError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub text: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorAnalysis {
    pub root_cause: String,
    pub category: ErrorCategory,
    pub severity: Severity,
    pub suggestions: Vec<Suggestion>,
    pub patterns: Vec<String>,
    pub confidence: f64,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct ContextualErrorAnalyzer;

impl ContextualErrorAnalyzer {
    pub fn new() -> Self {
        Self
    }

    pub fn analyze_error_context(
        &self,
        error_content: &str,
        context: &str,
    ) -> anyhow::Result<ErrorAnalysis> {
        if error_content.trim().len() < 10 {
            return Ok(ErrorAnalysis {
                root_cause: "No error content provided".to_string(),
                category: ErrorCategory::NoError,
                severity: Severity::Info,
                suggestions: vec![],
                patterns: vec![],
                confidence: 0.0,
            });
        }
        let safe = sanitize_content(error_content, 5000);
        let _safe_ctx = sanitize_content(context, 1000);
        let lower = safe.to_ascii_lowercase();

        if let Some((entry, score)) = patterns::match_best(&lower) {
            let confidence = (0.4_f64 + (score as f64 * 0.1)).min(0.8);
            let primary = filter_pattern_suggestion(entry.suggestion);
            let mut suggestions = vec![Suggestion {
                text: primary,
                confidence,
            }];
            for s in entry.steps {
                suggestions.push(Suggestion {
                    text: filter_pattern_suggestion(s),
                    confidence: confidence * 0.8,
                });
            }
            let matched: Vec<String> = entry
                .keywords
                .iter()
                .filter(|k| lower.contains(**k))
                .map(|s| (*s).to_string())
                .collect();
            return Ok(ErrorAnalysis {
                root_cause: format!("Detected {:?} based on error patterns", entry.category),
                category: entry.category,
                severity: entry.severity,
                suggestions,
                patterns: vec![format!("Pattern: {}", matched.join(" OR "))],
                confidence,
            });
        }

        // No keyword match: classify as Unknown with a generic suggestion.
        Ok(ErrorAnalysis {
            root_cause: "Generic error detected - requires manual investigation".to_string(),
            category: ErrorCategory::Unknown,
            severity: Severity::Low,
            suggestions: vec![],
            patterns: vec![],
            confidence: 0.3,
        })
    }

    /// Convenience: get the highest-confidence suggestion (if any).
    pub fn top_suggestion(
        &self,
        error_content: &str,
        context: &str,
    ) -> anyhow::Result<Option<Suggestion>> {
        let a = self.analyze_error_context(error_content, context)?;
        Ok(a.suggestions.into_iter().max_by(|x, y| {
            x.confidence
                .partial_cmp(&y.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        }))
    }
}
