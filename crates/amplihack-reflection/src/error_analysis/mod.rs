//! Contextual error analysis (Rust port of `contextual_error_analyzer.py`).

mod analyzer;
mod patterns;

pub use analyzer::{ContextualErrorAnalyzer, ErrorAnalysis, ErrorCategory, Severity, Suggestion};
