//! Teaching subprocess for L7 teacher-student evaluation.
//!
//! Ports Python `amplihack/eval/teaching_subprocess.py`:
//! - Subprocess-isolated teaching phase
//! - Knowledge base ingestion + lesson generation
//! - Status reporting with metrics

use serde::{Deserialize, Serialize};

/// Status of a teaching phase execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeachingPhaseResult {
    pub status: String,
    pub turns: usize,
    #[serde(default)]
    pub lesson_length: usize,
    #[serde(default)]
    pub total_facts: usize,
    #[serde(default)]
    pub error: Option<String>,
}

/// Configuration for a teaching subprocess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeachingSubprocessConfig {
    pub agent_name: String,
    pub max_turns: usize,
    #[serde(default = "default_model")]
    pub model: String,
}

fn default_model() -> String {
    "claude-opus-4-6".to_string()
}

impl Default for TeachingSubprocessConfig {
    fn default() -> Self {
        Self {
            agent_name: "eval-agent".to_string(),
            max_turns: 4,
            model: default_model(),
        }
    }
}

/// Run teaching phase: teacher teaches student using knowledge base.
///
/// Structural port — actual LLM-based teaching requires agent integration.
/// This implementation performs deterministic knowledge ingestion and
/// generates a structural lesson from the knowledge base.
pub fn teaching_phase(
    knowledge_base: &[String],
    config: &TeachingSubprocessConfig,
) -> TeachingPhaseResult {
    if knowledge_base.is_empty() {
        return TeachingPhaseResult {
            status: "error".to_string(),
            turns: 0,
            lesson_length: 0,
            total_facts: 0,
            error: Some("Empty knowledge base".to_string()),
        };
    }

    // Structural lesson: enumerate key concepts
    let lesson = knowledge_base
        .iter()
        .enumerate()
        .map(|(i, fact)| {
            let preview: String = fact.chars().take(200).collect();
            format!("{}. {}", i + 1, preview)
        })
        .collect::<Vec<_>>()
        .join("\n");

    TeachingPhaseResult {
        status: "success".to_string(),
        turns: config.max_turns.min(knowledge_base.len()),
        lesson_length: lesson.len(),
        total_facts: knowledge_base.len(),
        error: None,
    }
}

/// Input format for subprocess stdin (matches Python's json.load(sys.stdin)).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubprocessInput {
    pub knowledge_base: Vec<String>,
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
}

fn default_max_turns() -> usize {
    4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teaching_phase_success() {
        let kb = vec![
            "L1 tests direct recall.".to_string(),
            "L2 tests inference.".to_string(),
            "L3 tests synthesis.".to_string(),
        ];
        let config = TeachingSubprocessConfig::default();
        let result = teaching_phase(&kb, &config);

        assert_eq!(result.status, "success");
        assert_eq!(result.total_facts, 3);
        assert!(result.lesson_length > 0);
        assert!(result.error.is_none());
    }

    #[test]
    fn teaching_phase_empty_kb() {
        let config = TeachingSubprocessConfig::default();
        let result = teaching_phase(&[], &config);

        assert_eq!(result.status, "error");
        assert_eq!(result.turns, 0);
        assert!(result.error.is_some());
    }

    #[test]
    fn config_default() {
        let cfg = TeachingSubprocessConfig::default();
        assert_eq!(cfg.max_turns, 4);
        assert!(cfg.model.contains("claude"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = TeachingSubprocessConfig {
            agent_name: "test-agent".to_string(),
            max_turns: 6,
            model: "gpt-4".to_string(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: TeachingSubprocessConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agent_name, "test-agent");
        assert_eq!(back.max_turns, 6);
    }

    #[test]
    fn result_serde_roundtrip() {
        let r = TeachingPhaseResult {
            status: "success".to_string(),
            turns: 3,
            lesson_length: 150,
            total_facts: 5,
            error: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: TeachingPhaseResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.turns, 3);
    }

    #[test]
    fn subprocess_input_serde() {
        let input = SubprocessInput {
            knowledge_base: vec!["fact1".to_string()],
            max_turns: 3,
        };
        let json = serde_json::to_string(&input).unwrap();
        let back: SubprocessInput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.knowledge_base.len(), 1);
    }
}
