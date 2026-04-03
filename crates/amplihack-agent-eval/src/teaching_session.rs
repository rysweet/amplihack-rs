//! Multi-turn teacher-student session framework.
//!
//! Ports Python `amplihack/eval/teaching_session.py`:
//! - Structured dialogue where a teacher transfers knowledge to a student
//! - Self-explanation prompts for metacognition evaluation (Chi 1994)
//! - Stateless turns accumulated via history list
//! - Pluggable message generation via [`MessageGenerator`] trait

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a teaching session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeachingSessionConfig {
    /// Number of teacher-student exchanges.
    pub max_turns: usize,
    /// LLM model identifier (informational — actual generation delegated to
    /// [`MessageGenerator`]).
    #[serde(default = "default_model")]
    pub model: String,
    /// System prompt for the teacher role.
    #[serde(default = "default_teacher_prompt")]
    pub teacher_system_prompt: String,
    /// System prompt for the student role.
    #[serde(default = "default_student_prompt")]
    pub student_system_prompt: String,
}

fn default_model() -> String {
    "claude-opus-4-6".to_string()
}

fn default_teacher_prompt() -> String {
    "You are an expert teacher. Your job is to teach the student about a topic \
     using the knowledge base provided. Each turn, teach one or two key concepts. \
     Build on what the student already knows. Be concise and clear."
        .to_string()
}

fn default_student_prompt() -> String {
    "You are a student learning a new topic. After each teaching message, \
     respond with your understanding and explain your reasoning. \
     Always respond with a JSON object: \
     {\"response\": \"your understanding\", \"self_explanation\": \"why you think this is correct\"}"
        .to_string()
}

impl Default for TeachingSessionConfig {
    fn default() -> Self {
        Self {
            max_turns: 6,
            model: default_model(),
            teacher_system_prompt: default_teacher_prompt(),
            student_system_prompt: default_student_prompt(),
        }
    }
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// One turn of teacher-student dialogue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionTurn {
    /// Sequential turn number (1-indexed).
    pub turn_number: usize,
    /// What the teacher taught.
    pub teacher_message: String,
    /// Student's response.
    pub student_response: String,
    /// Student's self-explanation of understanding.
    pub self_explanation: String,
}

/// A history entry for context accumulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub role: String,
    pub content: String,
}

/// Result of a complete teaching session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeachingSessionResult {
    /// All dialogue turns.
    pub turns: Vec<SessionTurn>,
    /// Key concepts the teacher covered (first 200 chars of each teaching message).
    pub knowledge_transferred: Vec<String>,
    /// Rough estimate of student understanding (0.0–1.0).
    pub student_accuracy: f64,
}

// ---------------------------------------------------------------------------
// Message generator trait
// ---------------------------------------------------------------------------

/// Trait for pluggable message generation.
///
/// In production, implementations call an LLM. For testing, a deterministic
/// stub can be used.
pub trait MessageGenerator {
    /// Generate the teacher's message for a given turn.
    ///
    /// # Arguments
    /// * `turn_number` – current turn (1-indexed)
    /// * `max_turns` – total turns configured
    /// * `knowledge_base` – facts to teach from
    /// * `history` – previous dialogue entries
    /// * `system_prompt` – teacher system prompt
    fn generate_teacher_message(
        &self,
        turn_number: usize,
        max_turns: usize,
        knowledge_base: &[String],
        history: &[HistoryEntry],
        system_prompt: &str,
    ) -> Result<String, String>;

    /// Generate the student's response with self-explanation.
    ///
    /// Returns `(response, self_explanation)`.
    fn generate_student_response(
        &self,
        teacher_message: &str,
        history: &[HistoryEntry],
        system_prompt: &str,
    ) -> Result<(String, String), String>;
}

// ---------------------------------------------------------------------------
// Deterministic generator (for tests / offline use)
// ---------------------------------------------------------------------------

/// A deterministic message generator that echoes knowledge-base items.
///
/// Useful for testing session orchestration without LLM calls.
#[derive(Debug)]
pub struct DeterministicGenerator;

impl MessageGenerator for DeterministicGenerator {
    fn generate_teacher_message(
        &self,
        turn_number: usize,
        _max_turns: usize,
        knowledge_base: &[String],
        _history: &[HistoryEntry],
        _system_prompt: &str,
    ) -> Result<String, String> {
        let idx = (turn_number - 1) % knowledge_base.len().max(1);
        let fact = knowledge_base
            .get(idx)
            .cloned()
            .unwrap_or_else(|| format!("Concept {turn_number}"));
        Ok(format!("Let me teach you about: {fact}"))
    }

    fn generate_student_response(
        &self,
        teacher_message: &str,
        _history: &[HistoryEntry],
        _system_prompt: &str,
    ) -> Result<(String, String), String> {
        let response = format!("I understand: {teacher_message}");
        let explanation =
            format!("This makes sense because the teacher explained: {teacher_message}");
        Ok((response, explanation))
    }
}

// ---------------------------------------------------------------------------
// Teaching session
// ---------------------------------------------------------------------------

/// Orchestrates multi-turn teacher-student dialogue.
///
/// Creates a structured teaching session where a teacher agent transfers
/// knowledge from a provided knowledge base to a student agent. The student
/// provides self-explanations for each response to enable metacognition
/// evaluation.
pub struct TeachingSession<G: MessageGenerator> {
    knowledge_base: Vec<String>,
    config: TeachingSessionConfig,
    generator: G,
}

impl<G: MessageGenerator> std::fmt::Debug for TeachingSession<G> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeachingSession")
            .field("knowledge_base", &self.knowledge_base)
            .field("config", &self.config)
            .field("generator", &"<MessageGenerator>")
            .finish()
    }
}

impl<G: MessageGenerator> TeachingSession<G> {
    /// Create a new teaching session.
    ///
    /// # Errors
    /// Returns `Err` if `knowledge_base` is empty.
    pub fn new(
        knowledge_base: Vec<String>,
        config: TeachingSessionConfig,
        generator: G,
    ) -> Result<Self, String> {
        if knowledge_base.is_empty() {
            return Err("knowledge_base cannot be empty".to_string());
        }
        Ok(Self {
            knowledge_base,
            config,
            generator,
        })
    }

    /// Run the full teaching session.
    pub fn run(&self) -> TeachingSessionResult {
        let mut turns = Vec::new();
        let mut history: Vec<HistoryEntry> = Vec::new();
        let mut knowledge_transferred = Vec::new();

        for turn_num in 1..=self.config.max_turns {
            // Teacher generates a teaching message
            let teacher_msg = match self.generator.generate_teacher_message(
                turn_num,
                self.config.max_turns,
                &self.knowledge_base,
                &history,
                &self.config.teacher_system_prompt,
            ) {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::warn!("Turn {} teacher generation failed: {}", turn_num, e);
                    break;
                }
            };

            // Student responds with self-explanation
            let (student_resp, self_explanation) = match self.generator.generate_student_response(
                &teacher_msg,
                &history,
                &self.config.student_system_prompt,
            ) {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!("Turn {} student generation failed: {}", turn_num, e);
                    break;
                }
            };

            let turn = SessionTurn {
                turn_number: turn_num,
                teacher_message: teacher_msg.clone(),
                student_response: student_resp.clone(),
                self_explanation,
            };
            turns.push(turn);

            // Update history for context accumulation
            history.push(HistoryEntry {
                role: "teacher".to_string(),
                content: teacher_msg.clone(),
            });
            history.push(HistoryEntry {
                role: "student".to_string(),
                content: student_resp,
            });

            // Track what was taught (first 200 chars)
            let truncated: String = teacher_msg.chars().take(200).collect();
            knowledge_transferred.push(truncated);
        }

        let accuracy = estimate_accuracy(&turns);

        TeachingSessionResult {
            turns,
            knowledge_transferred,
            student_accuracy: accuracy,
        }
    }

    /// Access the knowledge base.
    pub fn knowledge_base(&self) -> &[String] {
        &self.knowledge_base
    }

    /// Access the config.
    pub fn config(&self) -> &TeachingSessionConfig {
        &self.config
    }
}

/// Convenience constructor for deterministic (offline) sessions.
impl TeachingSession<DeterministicGenerator> {
    /// Create a session with the deterministic generator (no LLM required).
    pub fn deterministic(
        knowledge_base: Vec<String>,
        config: TeachingSessionConfig,
    ) -> Result<Self, String> {
        Self::new(knowledge_base, config, DeterministicGenerator)
    }
}

// ---------------------------------------------------------------------------
// Accuracy estimation
// ---------------------------------------------------------------------------

/// Rough estimate of student understanding based on self-explanations.
///
/// Turns with non-empty self-explanations (>10 chars) score 1.0,
/// turns with only a response (>10 chars) score 0.5, otherwise 0.0.
pub fn estimate_accuracy(turns: &[SessionTurn]) -> f64 {
    if turns.is_empty() {
        return 0.0;
    }

    let total: f64 = turns
        .iter()
        .map(|t| {
            if t.self_explanation.trim().len() > 10 {
                1.0
            } else if t.student_response.trim().len() > 10 {
                0.5
            } else {
                0.0
            }
        })
        .sum();

    total / turns.len() as f64
}

// ---------------------------------------------------------------------------
// JSON response parsing (mirrors Python's _generate_student_response parser)
// ---------------------------------------------------------------------------

/// Parse a student JSON response into (response, self_explanation).
///
/// Handles raw JSON and markdown-fenced JSON blocks.
pub fn parse_student_json(raw: &str) -> (String, String) {
    // Try direct parse
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
        let response = parsed
            .get("response")
            .and_then(|v| v.as_str())
            .unwrap_or(raw)
            .to_string();
        let explanation = parsed
            .get("self_explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        return (response, explanation);
    }

    // Try markdown code block
    if let Some(start) = raw.find("```json") {
        let json_start = start + 7;
        if let Some(end) = raw[json_start..].find("```") {
            let json_str = raw[json_start..json_start + end].trim();
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                let response = parsed
                    .get("response")
                    .and_then(|v| v.as_str())
                    .unwrap_or(raw)
                    .to_string();
                let explanation = parsed
                    .get("self_explanation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                return (response, explanation);
            }
        }
    }

    // Fallback
    (raw.to_string(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default() {
        let cfg = TeachingSessionConfig::default();
        assert_eq!(cfg.max_turns, 6);
        assert!(cfg.model.contains("claude"));
        assert!(!cfg.teacher_system_prompt.is_empty());
        assert!(!cfg.student_system_prompt.is_empty());
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = TeachingSessionConfig {
            max_turns: 4,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: TeachingSessionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_turns, 4);
    }

    #[test]
    fn empty_knowledge_base_rejected() {
        let result = TeachingSession::deterministic(vec![], TeachingSessionConfig::default());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn deterministic_session_runs() {
        let kb = vec![
            "L1 tests direct recall.".to_string(),
            "L2 tests inference.".to_string(),
        ];
        let cfg = TeachingSessionConfig {
            max_turns: 3,
            ..Default::default()
        };
        let session = TeachingSession::deterministic(kb, cfg).unwrap();
        let result = session.run();

        assert_eq!(result.turns.len(), 3);
        assert_eq!(result.knowledge_transferred.len(), 3);
        // Deterministic generator always produces explanations > 10 chars
        assert!((result.student_accuracy - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn turn_numbers_are_sequential() {
        let kb = vec!["fact one".to_string(), "fact two".to_string()];
        let cfg = TeachingSessionConfig {
            max_turns: 4,
            ..Default::default()
        };
        let session = TeachingSession::deterministic(kb, cfg).unwrap();
        let result = session.run();

        for (i, turn) in result.turns.iter().enumerate() {
            assert_eq!(turn.turn_number, i + 1);
        }
    }

    #[test]
    fn knowledge_wraps_around() {
        let kb = vec!["only fact".to_string()];
        let cfg = TeachingSessionConfig {
            max_turns: 3,
            ..Default::default()
        };
        let session = TeachingSession::deterministic(kb, cfg).unwrap();
        let result = session.run();

        // All 3 turns should reference "only fact" via wrap-around
        for turn in &result.turns {
            assert!(turn.teacher_message.contains("only fact"));
        }
    }

    #[test]
    fn knowledge_transferred_truncated() {
        let long_fact = "x".repeat(300);
        let kb = vec![long_fact];
        let cfg = TeachingSessionConfig {
            max_turns: 1,
            ..Default::default()
        };
        let session = TeachingSession::deterministic(kb, cfg).unwrap();
        let result = session.run();

        assert!(result.knowledge_transferred[0].len() <= 200);
    }

    #[test]
    fn estimate_accuracy_empty() {
        assert!((estimate_accuracy(&[]) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_accuracy_all_good() {
        let turns = vec![SessionTurn {
            turn_number: 1,
            teacher_message: "teach".to_string(),
            student_response: "I learned something".to_string(),
            self_explanation: "This is correct because of reasons".to_string(),
        }];
        assert!((estimate_accuracy(&turns) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_accuracy_no_explanation() {
        let turns = vec![SessionTurn {
            turn_number: 1,
            teacher_message: "teach".to_string(),
            student_response: "I learned something useful here".to_string(),
            self_explanation: "".to_string(),
        }];
        assert!((estimate_accuracy(&turns) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_accuracy_empty_response() {
        let turns = vec![SessionTurn {
            turn_number: 1,
            teacher_message: "teach".to_string(),
            student_response: "".to_string(),
            self_explanation: "".to_string(),
        }];
        assert!((estimate_accuracy(&turns) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_student_json_valid() {
        let raw = r#"{"response": "I get it", "self_explanation": "because reasons"}"#;
        let (resp, expl) = parse_student_json(raw);
        assert_eq!(resp, "I get it");
        assert_eq!(expl, "because reasons");
    }

    #[test]
    fn parse_student_json_markdown() {
        let raw = "Here is my answer:\n```json\n{\"response\": \"yes\", \"self_explanation\": \"ok\"}\n```";
        let (resp, expl) = parse_student_json(raw);
        assert_eq!(resp, "yes");
        assert_eq!(expl, "ok");
    }

    #[test]
    fn parse_student_json_fallback() {
        let raw = "Just plain text response";
        let (resp, expl) = parse_student_json(raw);
        assert_eq!(resp, "Just plain text response");
        assert!(expl.is_empty());
    }

    #[test]
    fn session_turn_serde_roundtrip() {
        let turn = SessionTurn {
            turn_number: 1,
            teacher_message: "teach".to_string(),
            student_response: "learn".to_string(),
            self_explanation: "because".to_string(),
        };
        let json = serde_json::to_string(&turn).unwrap();
        let back: SessionTurn = serde_json::from_str(&json).unwrap();
        assert_eq!(turn, back);
    }

    #[test]
    fn session_result_serde_roundtrip() {
        let result = TeachingSessionResult {
            turns: vec![],
            knowledge_transferred: vec!["fact".to_string()],
            student_accuracy: 0.75,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: TeachingSessionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.student_accuracy, 0.75);
        assert_eq!(back.knowledge_transferred, vec!["fact"]);
    }

    // Custom generator that fails on turn 2
    struct FailingGenerator;

    impl MessageGenerator for FailingGenerator {
        fn generate_teacher_message(
            &self,
            turn_number: usize,
            _max_turns: usize,
            _knowledge_base: &[String],
            _history: &[HistoryEntry],
            _system_prompt: &str,
        ) -> Result<String, String> {
            if turn_number >= 2 {
                return Err("network error".to_string());
            }
            Ok("Teaching concept 1".to_string())
        }

        fn generate_student_response(
            &self,
            _teacher_message: &str,
            _history: &[HistoryEntry],
            _system_prompt: &str,
        ) -> Result<(String, String), String> {
            Ok(("Got it".to_string(), "Makes sense to me".to_string()))
        }
    }

    #[test]
    fn session_stops_on_error() {
        let kb = vec!["fact".to_string()];
        let cfg = TeachingSessionConfig {
            max_turns: 5,
            ..Default::default()
        };
        let session = TeachingSession::new(kb, cfg, FailingGenerator).unwrap();
        let result = session.run();

        // Should stop after turn 1 because turn 2 fails
        assert_eq!(result.turns.len(), 1);
        assert_eq!(result.turns[0].turn_number, 1);
    }

    #[test]
    fn accessors_work() {
        let kb = vec!["fact".to_string()];
        let cfg = TeachingSessionConfig {
            max_turns: 2,
            ..Default::default()
        };
        let session = TeachingSession::deterministic(kb.clone(), cfg).unwrap();
        assert_eq!(session.knowledge_base(), &kb);
        assert_eq!(session.config().max_turns, 2);
    }
}
