//! Meeting Synthesizer domain agent.
//!
//! Ports `domain_agents/meeting_synthesizer/agent.py`: MeetingSynthesizerAgent
//! that synthesizes meeting transcripts into structured outputs.

pub mod eval_levels;
pub mod tools;

use std::collections::HashMap;

use crate::base::{DomainAgent, DomainTeachingResult, EvalLevel, TaskResult};
use crate::error::Result;

use tools::{extract_action_items, generate_summary, identify_decisions, identify_topics};

const DEFAULT_PROMPT: &str = "You are an expert meeting synthesizer.";

/// Agent that synthesizes meeting transcripts into structured outputs.
pub struct MeetingSynthesizerAgent {
    agent_name: String,
    model: String,
}

impl MeetingSynthesizerAgent {
    pub fn new(agent_name: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
            model: model.into(),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new("meeting_synthesizer_agent", "gpt-4o-mini")
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn extract_actions(&self, transcript: &str) -> TaskResult {
        let items = extract_action_items(transcript);
        let count = items.len();
        TaskResult::ok_with_meta(
            serde_json::json!({"action_items": items, "action_count": count}),
            HashMap::from([("task_type".into(), serde_json::json!("extract_actions"))]),
        )
    }

    fn summarize(&self, transcript: &str) -> TaskResult {
        let summary = generate_summary(transcript);
        let participants = summary
            .get("participants")
            .cloned()
            .unwrap_or(serde_json::json!([]));
        let word_count = summary
            .get("word_count")
            .cloned()
            .unwrap_or(serde_json::json!(0));
        TaskResult::ok_with_meta(
            serde_json::json!({
                "summary": summary,
                "participants": participants,
                "word_count": word_count,
            }),
            HashMap::from([("task_type".into(), serde_json::json!("summarize"))]),
        )
    }

    fn identify_speakers_task(&self, transcript: &str) -> TaskResult {
        let summary = generate_summary(transcript);
        let speakers = summary
            .get("participants")
            .cloned()
            .unwrap_or(serde_json::json!([]));
        let count = speakers.as_array().map(|a| a.len()).unwrap_or(0);
        TaskResult::ok_with_meta(
            serde_json::json!({"speakers": speakers, "speaker_count": count}),
            HashMap::from([("task_type".into(), serde_json::json!("identify_speakers"))]),
        )
    }

    fn full_synthesis(&self, transcript: &str) -> TaskResult {
        let action_items = extract_action_items(transcript);
        let summary = generate_summary(transcript);
        let decisions = identify_decisions(transcript);
        let topics = identify_topics(transcript);

        let participants = summary
            .get("participants")
            .cloned()
            .unwrap_or(serde_json::json!([]));

        TaskResult::ok_with_meta(
            serde_json::json!({
                "action_items": action_items,
                "action_count": action_items.len(),
                "summary": summary,
                "decisions": decisions,
                "decision_count": decisions.len(),
                "topics": topics,
                "topic_count": topics.len(),
                "participants": participants,
            }),
            HashMap::from([("task_type".into(), serde_json::json!("full_synthesis"))]),
        )
    }
}

impl DomainAgent for MeetingSynthesizerAgent {
    fn domain(&self) -> &str {
        "meeting_synthesizer"
    }

    fn agent_name(&self) -> &str {
        &self.agent_name
    }

    fn system_prompt(&self) -> String {
        DEFAULT_PROMPT.to_string()
    }

    fn execute_task(&self, task: &HashMap<String, serde_json::Value>) -> Result<TaskResult> {
        let transcript = tools::get_str(task, "transcript");
        let task_type = task
            .get("task_type")
            .and_then(|v| v.as_str())
            .unwrap_or("full_synthesis");

        if transcript.trim().is_empty() {
            return Ok(TaskResult::fail("No transcript provided"));
        }

        Ok(match task_type {
            "extract_actions" => self.extract_actions(transcript),
            "summarize" => self.summarize(transcript),
            "identify_speakers" => self.identify_speakers_task(transcript),
            "full_synthesis" => self.full_synthesis(transcript),
            other => TaskResult::fail(format!("Unknown task_type: {other}")),
        })
    }

    fn eval_levels(&self) -> Vec<EvalLevel> {
        eval_levels::get_eval_levels()
    }

    fn teach(&self, topic: &str, student_level: &str) -> Result<DomainTeachingResult> {
        let key = topic
            .split_whitespace()
            .next()
            .unwrap_or("action")
            .to_lowercase();

        let lesson_plan = match key.as_str() {
            "summary" | "summarize" => {
                "1. Meeting structure\n2. Key point identification\n3. Participant roles\n4. Concise writing\n5. Practice summarization"
            }
            "decision" | "decisions" => {
                "1. What constitutes a decision?\n2. Explicit vs implicit decisions\n3. Recording rationale\n4. Tracking follow-ups\n5. Practice identification"
            }
            _ => {
                "1. What are action items?\n2. Identifying owners\n3. Extracting deadlines\n4. Prioritization\n5. Practice extraction"
            }
        };
        let mut plan = lesson_plan.to_string();
        if student_level == "advanced" {
            plan.push_str("\n6. Advanced: Multi-threaded discussion analysis");
        }

        let instruction = match key.as_str() {
            "summary" | "summarize" => {
                "When summarizing meetings:\n\n1. **Participant List**: Always identify who was in the meeting\n2. **Key Decisions**: Highlight what was decided, not just discussed\n3. **Action Items**: Include a summary of assigned tasks\n4. **Duration & Scope**: Estimate meeting length and breadth of topics"
            }
            "decision" | "decisions" => {
                "When identifying decisions:\n\n1. **Decision Indicators**: 'decided', 'agreed', 'approved', 'let's go with'\n2. **Implicit Decisions**: Sometimes decisions are made without explicit language\n3. **Context Matters**: Record who made the decision and what alternatives were discussed\n4. **Follow-up Actions**: Decisions often generate action items"
            }
            _ => {
                "When extracting action items:\n\n1. **Owner Identification**: Look for direct assignments - 'Bob, can you...'\n2. **Deadline Extraction**: Look for temporal phrases - 'by Friday', 'next week'\n3. **Implicit Actions**: Watch for commitments without explicit assignment language\n4. **Priority Signals**: Words like 'urgent', 'first', 'critical' indicate priority"
            }
        };

        let practice_transcript = "Alice: Let's review the sprint goals.\nBob: I think we need to fix the login bug.\nAlice: Bob, can you fix the login bug by tomorrow?\nCharlie: I'll update the test suite by end of week.\n";

        let items = extract_action_items(practice_transcript);
        let attempt = if !items.is_empty() {
            let findings: Vec<String> = items
                .iter()
                .take(5)
                .map(|i| {
                    format!(
                        "- Action: {} (Owner: {})",
                        i.get("action").and_then(|v| v.as_str()).unwrap_or(""),
                        i.get("owner").and_then(|v| v.as_str()).unwrap_or("unknown")
                    )
                })
                .collect();
            format!("Student findings:\n{}", findings.join("\n"))
        } else {
            "Student: No major findings (needs more training on this topic)".to_string()
        };

        Ok(DomainTeachingResult {
            lesson_plan: plan,
            instruction: instruction.to_string(),
            student_questions: vec![
                format!("What should I look for when extracting {topic}?"),
                format!("Can you give me an example of a {topic} extraction?"),
            ],
            agent_answers: vec![
                format!(
                    "Focus on speaker assignment patterns and temporal indicators for {topic}."
                ),
                format!(
                    "A common {topic} pattern: 'Bob, can you do X by Y' -> Owner: Bob, Action: X, Deadline: Y."
                ),
            ],
            student_attempt: attempt,
            scores: HashMap::new(),
        })
    }

    fn available_tools(&self) -> Vec<String> {
        vec![
            "extract_action_items".into(),
            "generate_summary".into(),
            "identify_decisions".into(),
            "identify_topics".into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> MeetingSynthesizerAgent {
        MeetingSynthesizerAgent::with_defaults()
    }

    #[test]
    fn domain_and_name() {
        let a = agent();
        assert_eq!(a.domain(), "meeting_synthesizer");
        assert_eq!(a.agent_name(), "meeting_synthesizer_agent");
    }

    #[test]
    fn execute_empty_transcript() {
        let a = agent();
        let task = HashMap::from([("transcript".into(), serde_json::json!(""))]);
        let r = a.execute_task(&task).unwrap();
        assert!(!r.success);
    }

    #[test]
    fn execute_full_synthesis() {
        let transcript = "Alice: Bob, can you draft the spec by Friday?\nBob: Sure, I will have the draft ready by Friday.\nCharlie: We decided to use PostgreSQL.\n";
        let a = agent();
        let task = HashMap::from([
            ("transcript".into(), serde_json::json!(transcript)),
            ("task_type".into(), serde_json::json!("full_synthesis")),
        ]);
        let r = a.execute_task(&task).unwrap();
        assert!(r.success);
        let output = r.output.unwrap();
        assert!(output["action_count"].as_u64().unwrap() > 0);
        assert!(output["decision_count"].as_u64().unwrap() > 0);
    }

    #[test]
    fn execute_extract_actions() {
        let transcript = "Alice: Bob, can you fix the bug by Monday?\n";
        let a = agent();
        let task = HashMap::from([
            ("transcript".into(), serde_json::json!(transcript)),
            ("task_type".into(), serde_json::json!("extract_actions")),
        ]);
        let r = a.execute_task(&task).unwrap();
        assert!(r.success);
    }

    #[test]
    fn execute_summarize() {
        let transcript = "Alice: Hello.\nBob: Let's discuss the plan.\n";
        let a = agent();
        let task = HashMap::from([
            ("transcript".into(), serde_json::json!(transcript)),
            ("task_type".into(), serde_json::json!("summarize")),
        ]);
        let r = a.execute_task(&task).unwrap();
        assert!(r.success);
        let output = r.output.unwrap();
        assert!(output.get("participants").is_some());
    }

    #[test]
    fn execute_unknown_task_type() {
        let a = agent();
        let task = HashMap::from([
            ("transcript".into(), serde_json::json!("Hello")),
            ("task_type".into(), serde_json::json!("unknown")),
        ]);
        let r = a.execute_task(&task).unwrap();
        assert!(!r.success);
    }

    #[test]
    fn teach_action_items() {
        let a = agent();
        let r = a.teach("action extraction", "beginner").unwrap();
        assert!(r.lesson_plan.contains("action items"));
        assert!(!r.student_attempt.is_empty());
    }

    #[test]
    fn eval_levels_returned() {
        let a = agent();
        let levels = a.eval_levels();
        assert_eq!(levels.len(), 4);
    }
}
