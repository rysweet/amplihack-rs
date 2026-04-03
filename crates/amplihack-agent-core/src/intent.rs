//! Intent detection and classification.
//!
//! Matches Python `amplihack/agents/goal_seeking/intent_detector.py`:
//! - Intent enum: StoreContent, AnswerQuestion, ExecuteTask, Unknown
//! - IntentDetector: classify(input) → Intent

use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Intent
// ---------------------------------------------------------------------------

/// Classified intent of user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    /// Input is content to be stored in memory.
    StoreContent,
    /// Input is a question to be answered.
    AnswerQuestion,
    /// Input is a task/command to be executed.
    ExecuteTask,
    /// Intent could not be determined.
    Unknown,
}

impl fmt::Display for Intent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StoreContent => write!(f, "store_content"),
            Self::AnswerQuestion => write!(f, "answer_question"),
            Self::ExecuteTask => write!(f, "execute_task"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

impl Intent {
    /// Whether this intent requires memory access.
    pub fn needs_memory(&self) -> bool {
        matches!(self, Self::StoreContent | Self::AnswerQuestion)
    }

    /// Whether this intent is actionable (not Unknown).
    pub fn is_actionable(&self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

// ---------------------------------------------------------------------------
// IntentDetector
// ---------------------------------------------------------------------------

/// Classifies raw text input into an `Intent`.
///
/// Port of Python `IntentDetector`. The `classify` body is a `todo!()`
/// stub — tests come first.
#[allow(dead_code)] // Fields used once todo!() stubs are implemented
pub struct IntentDetector {
    /// Question-word prefixes used for heuristic detection.
    question_words: Vec<&'static str>,
    /// Command prefixes used for heuristic detection.
    command_words: Vec<&'static str>,
}

impl IntentDetector {
    pub fn new() -> Self {
        Self {
            question_words: vec![
                "what", "who", "where", "when", "why", "how", "which",
                "is", "are", "do", "does", "can", "could", "would", "should",
            ],
            command_words: vec![
                "run", "execute", "create", "delete", "build", "test",
                "deploy", "install", "fix", "update", "start", "stop",
            ],
        }
    }

    /// Classify the given input into an `Intent`.
    pub fn classify(&self, _input: &str) -> Intent {
        todo!("classify: detect intent from input text")
    }
}

impl Default for IntentDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_display() {
        assert_eq!(Intent::StoreContent.to_string(), "store_content");
        assert_eq!(Intent::AnswerQuestion.to_string(), "answer_question");
        assert_eq!(Intent::ExecuteTask.to_string(), "execute_task");
        assert_eq!(Intent::Unknown.to_string(), "unknown");
    }

    #[test]
    fn intent_serde_roundtrip() {
        let json = serde_json::to_string(&Intent::ExecuteTask).unwrap();
        assert_eq!(json, r#""execute_task""#);
        let parsed: Intent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Intent::ExecuteTask);
    }

    #[test]
    fn intent_needs_memory() {
        assert!(Intent::StoreContent.needs_memory());
        assert!(Intent::AnswerQuestion.needs_memory());
        assert!(!Intent::ExecuteTask.needs_memory());
        assert!(!Intent::Unknown.needs_memory());
    }

    #[test]
    fn intent_is_actionable() {
        assert!(Intent::StoreContent.is_actionable());
        assert!(Intent::AnswerQuestion.is_actionable());
        assert!(Intent::ExecuteTask.is_actionable());
        assert!(!Intent::Unknown.is_actionable());
    }
}
