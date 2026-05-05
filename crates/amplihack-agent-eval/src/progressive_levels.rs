//! Built-in progressive test level data.
//!
//! Provides pre-defined test scenarios with questions and articles for each
//! progressive level (L1–L12). These replace the Python
//! `amplihack_eval.data.progressive_levels` module, eliminating the Python
//! dependency entirely.

use crate::levels::TestLevel;
use crate::models::{TestCase, TestQuestion};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Level scenario descriptor
// ---------------------------------------------------------------------------

/// A complete scenario definition for a progressive evaluation level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelScenario {
    /// Unique identifier, e.g. "L1-recall".
    pub level_id: String,
    /// Human-readable name.
    pub level_name: String,
    /// Longer description of what this level tests.
    pub description: String,
    /// The underlying TestLevel enum variant.
    pub level: TestLevel,
    /// Source articles provided to the agent during the learning phase.
    pub articles: Vec<Article>,
    /// Evaluation questions with expected answers.
    pub questions: Vec<LevelQuestion>,
}

impl LevelScenario {
    /// Convert questions to [`TestCase`] values usable by `ProgressiveSuite`.
    pub fn to_test_cases(&self) -> Vec<TestCase> {
        self.questions
            .iter()
            .map(|q| {
                let tq = TestQuestion {
                    id: q.id.clone(),
                    question: q.text.clone(),
                    context: q.context.clone(),
                    level: self.level,
                };
                TestCase {
                    question: tq,
                    expected_answer: q.expected_answer.clone(),
                    tags: q.tags.clone(),
                }
            })
            .collect()
    }
}

/// A source article provided to the agent during the teaching phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub id: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// A question used in a level scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelQuestion {
    pub id: String,
    pub text: String,
    pub expected_answer: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// Built-in level data
// ---------------------------------------------------------------------------

/// All 12 core progressive levels with built-in test data.
pub fn all_levels() -> Vec<LevelScenario> {
    vec![
        l1_recall(),
        l2_multi_source_synthesis(),
        l3_temporal_reasoning(),
        l4_procedural_learning(),
        l5_contradiction_handling(),
        l6_incremental_learning(),
        l7_teacher_student(),
        l8_metacognition(),
        l9_causal_reasoning(),
        l10_counterfactual_reasoning(),
        l11_novel_skill_acquisition(),
        l12_far_transfer(),
    ]
}

/// Advanced levels (L9–L12): causal, counterfactual, novel skills, far transfer.
pub fn advanced_levels() -> Vec<LevelScenario> {
    vec![
        l9_causal_reasoning(),
        l10_counterfactual_reasoning(),
        l11_novel_skill_acquisition(),
        l12_far_transfer(),
    ]
}

/// Teacher-student levels (L7 only).
pub fn teacher_student_levels() -> Vec<LevelScenario> {
    vec![l7_teacher_student()]
}

/// Novel skill acquisition levels (L11 only).
pub fn novel_skill_levels() -> Vec<LevelScenario> {
    vec![l11_novel_skill_acquisition()]
}

/// Transfer learning levels (L12 only).
pub fn transfer_levels() -> Vec<LevelScenario> {
    vec![l12_far_transfer()]
}

// ---------------------------------------------------------------------------
// Per-level constructors
// ---------------------------------------------------------------------------

fn l1_recall() -> LevelScenario {
    LevelScenario {
        level_id: "L1-recall".into(),
        level_name: "L1 Recall".into(),
        description: TestLevel::L1Recall.description().into(),
        level: TestLevel::L1Recall,
        articles: vec![
            Article {
                id: "art-l1-1".into(),
                title: "Rust Memory Safety".into(),
                content: "Rust guarantees memory safety without a garbage collector through its ownership system. Each value has exactly one owner, and when the owner goes out of scope the value is dropped.".into(),
                source: "rust-book".into(),
                timestamp: None,
            },
            Article {
                id: "art-l1-2".into(),
                title: "Borrow Checker".into(),
                content: "The borrow checker enforces that references must always be valid. You can have either one mutable reference or any number of immutable references, but not both.".into(),
                source: "rust-book".into(),
                timestamp: None,
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L1-q1".into(),
                text: "How does Rust guarantee memory safety?".into(),
                expected_answer: "Through its ownership system where each value has exactly one owner".into(),
                context: None,
                tags: vec!["recall".into()],
            },
            LevelQuestion {
                id: "L1-q2".into(),
                text: "What rule does the borrow checker enforce about references?".into(),
                expected_answer: "You can have either one mutable reference or any number of immutable references, but not both".into(),
                context: None,
                tags: vec!["recall".into()],
            },
        ],
    }
}

fn l2_multi_source_synthesis() -> LevelScenario {
    LevelScenario {
        level_id: "L2-multi-source".into(),
        level_name: "L2 Multi-Source Synthesis".into(),
        description: TestLevel::L2MultiSourceSynthesis.description().into(),
        level: TestLevel::L2MultiSourceSynthesis,
        articles: vec![
            Article {
                id: "art-l2-1".into(),
                title: "Agent Architecture".into(),
                content: "Modern AI agents use a loop of observe-think-act. The agent receives observations from the environment, reasons about them, and selects actions.".into(),
                source: "agent-design".into(),
                timestamp: None,
            },
            Article {
                id: "art-l2-2".into(),
                title: "Tool Use in Agents".into(),
                content: "Agents can invoke external tools such as code execution, web search, and file I/O. Tool selection is guided by the agent's planning module.".into(),
                source: "tool-paper".into(),
                timestamp: None,
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L2-q1".into(),
                text: "Describe the full agent loop including tool use.".into(),
                expected_answer: "The agent observes, thinks, and acts using the observe-think-act loop. It can invoke external tools like code execution and web search, selected by its planning module.".into(),
                context: None,
                tags: vec!["synthesis".into()],
            },
        ],
    }
}

fn l3_temporal_reasoning() -> LevelScenario {
    LevelScenario {
        level_id: "L3-temporal".into(),
        level_name: "L3 Temporal Reasoning".into(),
        description: TestLevel::L3TemporalReasoning.description().into(),
        level: TestLevel::L3TemporalReasoning,
        articles: vec![
            Article {
                id: "art-l3-1".into(),
                title: "Project Timeline".into(),
                content: "Phase 1 (Jan): requirements gathering. Phase 2 (Mar): design. Phase 3 (Jun): implementation. Phase 4 (Sep): testing. Phase 5 (Nov): deployment.".into(),
                source: "project-plan".into(),
                timestamp: Some("2025-01-01".into()),
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L3-q1".into(),
                text: "What phase comes before implementation and after requirements?".into(),
                expected_answer: "Design phase, which occurs in March, after requirements in January and before implementation in June".into(),
                context: None,
                tags: vec!["temporal".into()],
            },
        ],
    }
}

fn l4_procedural_learning() -> LevelScenario {
    LevelScenario {
        level_id: "L4-procedural".into(),
        level_name: "L4 Procedural Learning".into(),
        description: TestLevel::L4ProceduralLearning.description().into(),
        level: TestLevel::L4ProceduralLearning,
        articles: vec![
            Article {
                id: "art-l4-1".into(),
                title: "Deployment Procedure".into(),
                content: "Step 1: Run cargo test. Step 2: Run cargo clippy --all-targets. Step 3: Build release binary with cargo build --release. Step 4: Tag the release. Step 5: Push tag and binary to registry.".into(),
                source: "runbook".into(),
                timestamp: None,
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L4-q1".into(),
                text: "What are the deployment steps in order?".into(),
                expected_answer: "1. Run cargo test, 2. Run cargo clippy, 3. Build release binary, 4. Tag the release, 5. Push tag and binary to registry".into(),
                context: None,
                tags: vec!["procedural".into()],
            },
        ],
    }
}

fn l5_contradiction_handling() -> LevelScenario {
    LevelScenario {
        level_id: "L5-contradiction".into(),
        level_name: "L5 Contradiction Handling".into(),
        description: TestLevel::L5ContradictionHandling.description().into(),
        level: TestLevel::L5ContradictionHandling,
        articles: vec![
            Article {
                id: "art-l5-1".into(),
                title: "Config Format (v1)".into(),
                content: "The system uses YAML for all configuration files. YAML was chosen for its readability.".into(),
                source: "docs-v1".into(),
                timestamp: Some("2024-01-15".into()),
            },
            Article {
                id: "art-l5-2".into(),
                title: "Config Format (v2)".into(),
                content: "The system now uses TOML for configuration. TOML replaced YAML due to parsing ambiguities in YAML 1.1.".into(),
                source: "docs-v2".into(),
                timestamp: Some("2025-03-01".into()),
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L5-q1".into(),
                text: "What configuration format does the system use?".into(),
                expected_answer: "TOML (previously YAML, which was replaced due to parsing ambiguities). The newer v2 documentation is authoritative.".into(),
                context: None,
                tags: vec!["contradiction".into()],
            },
        ],
    }
}

fn l6_incremental_learning() -> LevelScenario {
    LevelScenario {
        level_id: "L6-incremental".into(),
        level_name: "L6 Incremental Learning".into(),
        description: TestLevel::L6IncrementalLearning.description().into(),
        level: TestLevel::L6IncrementalLearning,
        articles: vec![
            Article {
                id: "art-l6-1".into(),
                title: "Module A".into(),
                content: "Module A provides user authentication via JWT tokens.".into(),
                source: "arch-docs".into(),
                timestamp: None,
            },
            Article {
                id: "art-l6-2".into(),
                title: "Module B".into(),
                content: "Module B extends Module A by adding OAuth2 support and refresh token rotation.".into(),
                source: "arch-docs".into(),
                timestamp: None,
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L6-q1".into(),
                text: "What authentication capabilities does the system have after Module B?".into(),
                expected_answer: "JWT authentication from Module A plus OAuth2 support and refresh token rotation from Module B".into(),
                context: None,
                tags: vec!["incremental".into()],
            },
        ],
    }
}

fn l7_teacher_student() -> LevelScenario {
    LevelScenario {
        level_id: "L7-teacher-student".into(),
        level_name: "L7 Teacher-Student".into(),
        description: TestLevel::L7TeacherStudent.description().into(),
        level: TestLevel::L7TeacherStudent,
        articles: vec![
            Article {
                id: "art-l7-1".into(),
                title: "Error Handling Patterns".into(),
                content: "Use Result<T, E> for recoverable errors and panic! only for unrecoverable bugs. Map errors with .map_err() to provide context. Use the ? operator for ergonomic propagation.".into(),
                source: "best-practices".into(),
                timestamp: None,
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L7-q1".into(),
                text: "Explain Rust error handling to a beginner in simple terms.".into(),
                expected_answer: "Use Result for errors you can handle, panic for bugs. The ? operator passes errors up. Use map_err to add context to errors.".into(),
                context: None,
                tags: vec!["teaching".into()],
            },
        ],
    }
}

fn l8_metacognition() -> LevelScenario {
    LevelScenario {
        level_id: "L8-metacognition".into(),
        level_name: "L8 Metacognition".into(),
        description: TestLevel::L8Metacognition.description().into(),
        level: TestLevel::L8Metacognition,
        articles: vec![
            Article {
                id: "art-l8-1".into(),
                title: "System Limits".into(),
                content: "The system supports up to 10,000 concurrent connections. Beyond this, performance degrades. The database can hold 50 TB of data.".into(),
                source: "architecture".into(),
                timestamp: None,
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L8-q1".into(),
                text: "What do you know about the system's scalability, and what don't you know?".into(),
                expected_answer: "Known: supports 10,000 concurrent connections and 50 TB storage. Unknown: latency characteristics under load, geographic distribution limits, recovery time objectives.".into(),
                context: None,
                tags: vec!["metacognition".into()],
            },
        ],
    }
}

fn l9_causal_reasoning() -> LevelScenario {
    LevelScenario {
        level_id: "L9-causal".into(),
        level_name: "L9 Causal Reasoning".into(),
        description: TestLevel::L9CausalReasoning.description().into(),
        level: TestLevel::L9CausalReasoning,
        articles: vec![
            Article {
                id: "art-l9-1".into(),
                title: "Outage Report".into(),
                content: "The API outage on March 5 was caused by a misconfigured connection pool. The pool was set to max 5 connections, but the new feature required 20. This caused request queuing and eventual timeouts.".into(),
                source: "incident-report".into(),
                timestamp: Some("2025-03-06".into()),
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L9-q1".into(),
                text: "What was the root cause of the API outage and what chain of events followed?".into(),
                expected_answer: "Root cause: connection pool misconfigured to max 5 when 20 were needed. This caused request queuing, which led to timeouts and the outage.".into(),
                context: None,
                tags: vec!["causal".into()],
            },
        ],
    }
}

fn l10_counterfactual_reasoning() -> LevelScenario {
    LevelScenario {
        level_id: "L10-counterfactual".into(),
        level_name: "L10 Counterfactual Reasoning".into(),
        description: TestLevel::L10CounterfactualReasoning.description().into(),
        level: TestLevel::L10CounterfactualReasoning,
        articles: vec![
            Article {
                id: "art-l10-1".into(),
                title: "Architecture Decision Record: Monolith".into(),
                content: "The team chose a monolithic architecture for speed of delivery. Trade-offs considered: microservices would add operational complexity but improve independent deployability.".into(),
                source: "adr-001".into(),
                timestamp: None,
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L10-q1".into(),
                text: "What would have changed if the team had chosen microservices instead of a monolith?".into(),
                expected_answer: "Higher operational complexity but better independent deployability. Delivery speed would have been slower initially but scaling individual services would be easier.".into(),
                context: None,
                tags: vec!["counterfactual".into()],
            },
        ],
    }
}

fn l11_novel_skill_acquisition() -> LevelScenario {
    LevelScenario {
        level_id: "L11-novel-skill".into(),
        level_name: "L11 Novel Skill Acquisition".into(),
        description: TestLevel::L11NovelSkillAcquisition.description().into(),
        level: TestLevel::L11NovelSkillAcquisition,
        articles: vec![
            Article {
                id: "art-l11-1".into(),
                title: "Custom DSL for Config Validation".into(),
                content: "The validate_config DSL uses rules like: require 'name' type string; require 'port' type int range 1..65535; optional 'tls' type bool default false. Rules are evaluated top-to-bottom.".into(),
                source: "dsl-reference".into(),
                timestamp: None,
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L11-q1".into(),
                text: "Write a validate_config rule that requires a 'timeout' field as an integer between 1 and 300.".into(),
                expected_answer: "require 'timeout' type int range 1..300".into(),
                context: None,
                tags: vec!["novel-skill".into()],
            },
        ],
    }
}

fn l12_far_transfer() -> LevelScenario {
    LevelScenario {
        level_id: "L12-far-transfer".into(),
        level_name: "L12 Far Transfer".into(),
        description: TestLevel::L12FarTransfer.description().into(),
        level: TestLevel::L12FarTransfer,
        articles: vec![
            Article {
                id: "art-l12-1".into(),
                title: "Circuit Breaker Pattern".into(),
                content: "In software, a circuit breaker monitors failures. After a threshold is reached it opens and rejects requests immediately instead of waiting for timeouts. After a cooldown period it allows a probe request to test recovery.".into(),
                source: "patterns-book".into(),
                timestamp: None,
            },
        ],
        questions: vec![
            LevelQuestion {
                id: "L12-q1".into(),
                text: "How could the circuit breaker pattern apply to a hiring process?".into(),
                expected_answer: "If a recruiting channel consistently produces poor candidates (failures), stop investing in it (open circuit). After a period, try one candidate from that channel (probe) to see if quality improved before resuming.".into(),
                context: None,
                tags: vec!["far-transfer".into()],
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_levels_returns_12() {
        let levels = all_levels();
        assert_eq!(levels.len(), 12);
        for (i, level) in levels.iter().enumerate() {
            assert_eq!(level.level.id() as usize, i + 1);
            assert!(
                !level.questions.is_empty(),
                "Level {} has no questions",
                level.level_id
            );
            assert!(
                !level.articles.is_empty(),
                "Level {} has no articles",
                level.level_id
            );
        }
    }

    #[test]
    fn advanced_levels_are_l9_through_l12() {
        let levels = advanced_levels();
        assert_eq!(levels.len(), 4);
        assert_eq!(levels[0].level, TestLevel::L9CausalReasoning);
        assert_eq!(levels[3].level, TestLevel::L12FarTransfer);
    }

    #[test]
    fn to_test_cases_round_trips() {
        let scenario = l1_recall();
        let cases = scenario.to_test_cases();
        assert_eq!(cases.len(), scenario.questions.len());
        assert_eq!(cases[0].question.level, TestLevel::L1Recall);
    }

    #[test]
    fn level_ids_are_unique() {
        let levels = all_levels();
        let mut ids: Vec<&str> = levels.iter().map(|l| l.level_id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 12);
    }
}
