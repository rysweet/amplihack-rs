//! Evaluation levels for the Meeting Synthesizer agent.
//!
//! Ports `domain_agents/meeting_synthesizer/eval_levels.py`: L1-L4 evaluation
//! scenarios covering extraction, attribution, decisions, and synthesis.

use crate::base::{EvalLevel, EvalScenario};
use std::collections::HashMap;

const SIMPLE_TRANSCRIPT: &str = "\
Alice: Good morning. Let's discuss the Q1 roadmap.\n\
Bob: I think we should prioritize the API redesign.\n\
Alice: Agreed. Bob, can you draft the API spec by Friday?\n\
Bob: Sure, I will have the draft ready by Friday.\n\
Charlie: I need to finish the database migration first.\n\
Alice: Charlie, please complete the migration by next Wednesday.\n\
Alice: Let's meet again next Monday to review progress.\n";

const MULTI_SPEAKER: &str = "\
Alice: Welcome to the sprint retrospective.\n\
Bob: The deployment pipeline improvements reduced deploy time by 40%.\n\
Charlie: We had three incidents related to the new caching layer.\n\
Diana: I think the caching issues were because we didn't have enough testing.\n\
Alice: Diana, can you set up integration tests for the cache by end of sprint?\n\
Diana: Yes, I will write the integration test suite.\n\
Bob: We also need monitoring. I'll add cache monitoring dashboards by next Tuesday.\n\
Charlie: We decided to move from Redis to Valkey.\n\
Alice: Charlie, please update the ADR document.\n\
Alice: I'll get you staging access today.\n";

const DECISIONS_TRANSCRIPT: &str = "\
Alice: We need to choose a database.\n\
Bob: I vote for PostgreSQL.\n\
Charlie: After discussion, we decided to use PostgreSQL.\n\
Alice: Agreed. Let's go with PostgreSQL.\n\
Bob: We also decided to use Redis for caching.\n";

pub fn get_eval_levels() -> Vec<EvalLevel> {
    vec![l1(), l2(), l3(), l4()]
}

fn l1() -> EvalLevel {
    EvalLevel::new(
        "L1",
        "Basic Extraction",
        "Extracts action items and summaries from clear transcripts",
        vec![
            EvalScenario {
                scenario_id: "L1-001".into(),
                name: "Simple action extraction".into(),
                input_data: HashMap::from([
                    ("transcript".into(), serde_json::json!(SIMPLE_TRANSCRIPT)),
                    ("task_type".into(), serde_json::json!("full_synthesis")),
                ]),
                expected_output: HashMap::from([("min_action_count".into(), serde_json::json!(1))]),
                grading_rubric: "Must extract at least 1 action item and mention key participants."
                    .into(),
            },
            EvalScenario {
                scenario_id: "L1-002".into(),
                name: "Summary generation".into(),
                input_data: HashMap::from([
                    ("transcript".into(), serde_json::json!(SIMPLE_TRANSCRIPT)),
                    ("task_type".into(), serde_json::json!("summarize")),
                ]),
                expected_output: HashMap::from([("min_word_count".into(), serde_json::json!(1))]),
                grading_rubric: "Must generate a non-empty summary with participant names.".into(),
            },
            EvalScenario {
                scenario_id: "L1-003".into(),
                name: "Empty transcript handling".into(),
                input_data: HashMap::from([
                    ("transcript".into(), serde_json::json!("")),
                    ("task_type".into(), serde_json::json!("full_synthesis")),
                ]),
                expected_output: HashMap::new(),
                grading_rubric: "Must handle empty transcript gracefully without crashing.".into(),
            },
        ],
    )
    .with_threshold(0.6)
}

fn l2() -> EvalLevel {
    EvalLevel::new(
        "L2",
        "Attribution & Detail",
        "Correctly attributes actions and identifies deadlines",
        vec![
            EvalScenario {
                scenario_id: "L2-001".into(),
                name: "Owner attribution".into(),
                input_data: HashMap::from([
                    ("transcript".into(), serde_json::json!(SIMPLE_TRANSCRIPT)),
                    ("task_type".into(), serde_json::json!("extract_actions")),
                ]),
                expected_output: HashMap::from([("min_action_count".into(), serde_json::json!(2))]),
                grading_rubric: "Must attribute action items to correct owners.".into(),
            },
            EvalScenario {
                scenario_id: "L2-002".into(),
                name: "Deadline extraction".into(),
                input_data: HashMap::from([
                    ("transcript".into(), serde_json::json!(SIMPLE_TRANSCRIPT)),
                    ("task_type".into(), serde_json::json!("extract_actions")),
                ]),
                expected_output: HashMap::from([("min_action_count".into(), serde_json::json!(1))]),
                grading_rubric: "Must extract deadlines from action items.".into(),
            },
            EvalScenario {
                scenario_id: "L2-003".into(),
                name: "Multi-speaker identification".into(),
                input_data: HashMap::from([
                    ("transcript".into(), serde_json::json!(MULTI_SPEAKER)),
                    ("task_type".into(), serde_json::json!("identify_speakers")),
                ]),
                expected_output: HashMap::from([(
                    "min_speaker_count".into(),
                    serde_json::json!(4),
                )]),
                grading_rubric: "Must identify all 4 speakers.".into(),
            },
        ],
    )
    .with_threshold(0.6)
}

fn l3() -> EvalLevel {
    EvalLevel::new(
        "L3",
        "Decision Tracking",
        "Identifies decisions and key discussion points",
        vec![
            EvalScenario {
                scenario_id: "L3-001".into(),
                name: "Decision identification".into(),
                input_data: HashMap::from([
                    ("transcript".into(), serde_json::json!(DECISIONS_TRANSCRIPT)),
                    ("task_type".into(), serde_json::json!("full_synthesis")),
                ]),
                expected_output: HashMap::from([(
                    "min_decision_count".into(),
                    serde_json::json!(1),
                )]),
                grading_rubric: "Must identify the database decision.".into(),
            },
            EvalScenario {
                scenario_id: "L3-002".into(),
                name: "Topic identification".into(),
                input_data: HashMap::from([
                    ("transcript".into(), serde_json::json!(MULTI_SPEAKER)),
                    ("task_type".into(), serde_json::json!("full_synthesis")),
                ]),
                expected_output: HashMap::from([("min_topic_count".into(), serde_json::json!(1))]),
                grading_rubric: "Must identify at least one discussion topic.".into(),
            },
        ],
    )
    .with_threshold(0.6)
}

fn l4() -> EvalLevel {
    EvalLevel::new(
        "L4",
        "Complex Synthesis",
        "Handles complex multi-party meetings",
        vec![
            EvalScenario {
                scenario_id: "L4-001".into(),
                name: "Multi-party synthesis".into(),
                input_data: HashMap::from([
                    ("transcript".into(), serde_json::json!(MULTI_SPEAKER)),
                    ("task_type".into(), serde_json::json!("full_synthesis")),
                ]),
                expected_output: HashMap::from([("min_action_count".into(), serde_json::json!(2))]),
                grading_rubric: "Must synthesize multiple action items across different speakers."
                    .into(),
            },
            EvalScenario {
                scenario_id: "L4-002".into(),
                name: "Complete meeting analysis".into(),
                input_data: HashMap::from([
                    ("transcript".into(), serde_json::json!(MULTI_SPEAKER)),
                    ("task_type".into(), serde_json::json!("full_synthesis")),
                ]),
                expected_output: HashMap::from([
                    ("min_action_count".into(), serde_json::json!(2)),
                    ("min_decision_count".into(), serde_json::json!(1)),
                ]),
                grading_rubric: "Must capture actions, decisions, and technology mentions.".into(),
            },
        ],
    )
    .with_threshold(0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_levels_count() {
        let levels = get_eval_levels();
        assert_eq!(levels.len(), 4);
    }

    #[test]
    fn level_ids() {
        let levels = get_eval_levels();
        let ids: Vec<&str> = levels.iter().map(|l| l.level_id.as_str()).collect();
        assert_eq!(ids, vec!["L1", "L2", "L3", "L4"]);
    }

    #[test]
    fn l1_has_three_scenarios() {
        let levels = get_eval_levels();
        assert_eq!(levels[0].scenarios.len(), 3);
    }

    #[test]
    fn thresholds() {
        let levels = get_eval_levels();
        assert!((levels[0].passing_threshold - 0.6).abs() < f64::EPSILON);
        assert!((levels[3].passing_threshold - 0.5).abs() < f64::EPSILON);
    }
}
