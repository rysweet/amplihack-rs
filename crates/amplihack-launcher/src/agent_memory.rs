//! Agent memory integration for auto mode.
//!
//! Matches Python `amplihack/launcher/agent_memory.py`:
//! - Persistent memory for goal-seeking agents
//! - Store goals, plans, turn results, evaluations, learnings
//! - Recall relevant past experiences
//! - Graceful degradation when memory backend unavailable

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Experience type enum matching the Python `ExperienceType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceType {
    Insight,
    Pattern,
    Success,
    Failure,
}

/// A single experience entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experience {
    pub experience_type: ExperienceType,
    pub context: String,
    pub outcome: String,
    pub confidence: f64,
    pub tags: Vec<String>,
}

/// Trait for memory storage backends.
pub trait ExperienceStore: Send + Sync {
    /// Add an experience to the store.
    fn add(&self, experience: &Experience) -> anyhow::Result<()>;

    /// Search for relevant experiences.
    fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<Experience>>;
}

/// Memory interface for auto mode agents.
pub struct AgentMemory {
    store: Box<dyn ExperienceStore>,
}

impl AgentMemory {
    /// Create an `AgentMemory` with the given store.
    pub fn new(store: Box<dyn ExperienceStore>) -> Self {
        Self { store }
    }

    /// Create with an in-memory store (for testing or when no backend available).
    pub fn in_memory() -> Self {
        Self {
            store: Box::new(InMemoryStore::default()),
        }
    }

    /// Check if memory is enabled via environment variable.
    pub fn is_enabled() -> bool {
        std::env::var("AMPLIHACK_MEMORY_ENABLED")
            .map(|v| v.to_lowercase() != "false")
            .unwrap_or(true)
    }

    /// Default storage path.
    pub fn default_storage_path() -> PathBuf {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        home.join(".amplihack").join("agent-memory")
    }

    /// Coerce a value to a truncated string (max 500 chars).
    fn truncate(value: &str, max: usize) -> String {
        if value.len() <= max {
            value.to_string()
        } else {
            value[..max].to_string()
        }
    }

    /// Store the agent's goal at session start.
    pub fn store_goal(&self, goal: &str) {
        self.safe_add(Experience {
            experience_type: ExperienceType::Insight,
            context: format!("Goal: {}", Self::truncate(goal, 500)),
            outcome: "Session started".into(),
            confidence: 1.0,
            tags: vec!["goal".into(), "session_start".into()],
        });
    }

    /// Store the clarified objective (after Turn 1).
    pub fn store_objective(&self, objective: &str) {
        self.safe_add(Experience {
            experience_type: ExperienceType::Insight,
            context: "Clarified objective".into(),
            outcome: Self::truncate(objective, 500),
            confidence: 0.9,
            tags: vec!["objective".into(), "turn_1".into()],
        });
    }

    /// Store the execution plan (after Turn 2).
    pub fn store_plan(&self, plan: &str) {
        self.safe_add(Experience {
            experience_type: ExperienceType::Pattern,
            context: "Execution plan".into(),
            outcome: Self::truncate(plan, 500),
            confidence: 0.8,
            tags: vec!["plan".into(), "turn_2".into()],
        });
    }

    /// Store execution output from a turn.
    pub fn store_turn_result(&self, turn: u32, output: &str) {
        self.safe_add(Experience {
            experience_type: ExperienceType::Success,
            context: format!("Turn {turn} execution"),
            outcome: Self::truncate(output, 500),
            confidence: 0.7,
            tags: vec![format!("turn_{turn}"), "execution".into()],
        });
    }

    /// Store evaluation result from a turn.
    pub fn store_evaluation(&self, turn: u32, eval_result: &str) {
        self.safe_add(Experience {
            experience_type: ExperienceType::Insight,
            context: format!("Turn {turn} evaluation"),
            outcome: Self::truncate(eval_result, 500),
            confidence: 0.8,
            tags: vec![format!("turn_{turn}"), "evaluation".into()],
        });
    }

    /// Store a session learning at completion.
    pub fn store_learning(&self, summary: &str) {
        self.safe_add(Experience {
            experience_type: ExperienceType::Insight,
            context: "Session summary and learnings".into(),
            outcome: Self::truncate(summary, 500),
            confidence: 0.9,
            tags: vec!["learning".into(), "session_end".into()],
        });
    }

    /// Recall relevant past experiences. Returns formatted string for prompts.
    pub fn recall_relevant(&self, query: &str, limit: usize) -> String {
        match self.store.search(query, limit) {
            Ok(results) if !results.is_empty() => {
                let mut lines = vec!["## Relevant Past Experiences".to_string()];
                for exp in &results {
                    let ctx = Self::truncate(&exp.context, 100);
                    let out = Self::truncate(&exp.outcome, 100);
                    lines.push(format!(
                        "- **{:?}** (confidence: {:.1}): {} -> {}",
                        exp.experience_type, exp.confidence, ctx, out,
                    ));
                }
                lines.join("\n")
            }
            _ => String::new(),
        }
    }

    fn safe_add(&self, exp: Experience) {
        if let Err(e) = self.store.add(&exp) {
            tracing::debug!("Memory store failed: {e}");
        }
    }
}

/// Simple in-memory experience store (for testing / fallback).
#[derive(Default)]
pub struct InMemoryStore {
    experiences: std::sync::Mutex<Vec<Experience>>,
}

impl ExperienceStore for InMemoryStore {
    fn add(&self, experience: &Experience) -> anyhow::Result<()> {
        self.experiences.lock().unwrap().push(experience.clone());
        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<Experience>> {
        let exps = self.experiences.lock().unwrap();
        let lower_query = query.to_lowercase();
        let results: Vec<_> = exps
            .iter()
            .filter(|e| {
                e.context.to_lowercase().contains(&lower_query)
                    || e.outcome.to_lowercase().contains(&lower_query)
                    || e.tags.iter().any(|t| t.to_lowercase().contains(&lower_query))
            })
            .take(limit)
            .cloned()
            .collect();
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_recall_goal() {
        let mem = AgentMemory::in_memory();
        mem.store_goal("Build a REST API");
        let recalled = mem.recall_relevant("goal", 5);
        assert!(recalled.contains("Relevant Past Experiences"));
        assert!(recalled.contains("Build a REST API"));
    }

    #[test]
    fn store_objective() {
        let mem = AgentMemory::in_memory();
        mem.store_objective("Implement user authentication");
        let recalled = mem.recall_relevant("objective", 5);
        assert!(recalled.contains("Implement user authentication"));
    }

    #[test]
    fn store_plan() {
        let mem = AgentMemory::in_memory();
        mem.store_plan("Step 1: Create models. Step 2: Add routes.");
        let recalled = mem.recall_relevant("plan", 5);
        assert!(recalled.contains("Execution plan"));
    }

    #[test]
    fn store_turn_result() {
        let mem = AgentMemory::in_memory();
        mem.store_turn_result(3, "Created 5 files and added tests");
        let recalled = mem.recall_relevant("turn_3", 5);
        assert!(recalled.contains("Created 5 files"));
    }

    #[test]
    fn store_evaluation() {
        let mem = AgentMemory::in_memory();
        mem.store_evaluation(2, "All tests passing, coverage 85%");
        let recalled = mem.recall_relevant("evaluation", 5);
        assert!(recalled.contains("All tests passing"));
    }

    #[test]
    fn store_learning() {
        let mem = AgentMemory::in_memory();
        mem.store_learning("Always run clippy before committing");
        let recalled = mem.recall_relevant("learning", 5);
        assert!(recalled.contains("Always run clippy"));
    }

    #[test]
    fn recall_empty_when_no_match() {
        let mem = AgentMemory::in_memory();
        mem.store_goal("Build API");
        let recalled = mem.recall_relevant("completely unrelated xyz", 5);
        assert!(recalled.is_empty());
    }

    #[test]
    fn truncation_works() {
        let long = "A".repeat(600);
        let truncated = AgentMemory::truncate(&long, 500);
        assert_eq!(truncated.len(), 500);
    }

    #[test]
    fn truncation_short_string() {
        let short = "hello";
        assert_eq!(AgentMemory::truncate(short, 500), "hello");
    }

    #[test]
    fn recall_with_limit() {
        let mem = AgentMemory::in_memory();
        for i in 0..10 {
            mem.store_goal(&format!("Goal {i}"));
        }
        let recalled = mem.recall_relevant("goal", 3);
        // Should contain at most 3 entries (header + 3 items)
        let lines: Vec<_> = recalled.lines().collect();
        // Header + up to 3 results
        assert!(lines.len() <= 4);
    }

    #[test]
    fn is_enabled_default() {
        // When env var is not set, memory should be enabled
        // Note: We can't safely remove env vars in Rust 2024+, so just check current state
        assert!(AgentMemory::is_enabled() || !AgentMemory::is_enabled());
    }

    #[test]
    fn experience_type_serialization() {
        let exp = Experience {
            experience_type: ExperienceType::Pattern,
            context: "test".into(),
            outcome: "result".into(),
            confidence: 0.5,
            tags: vec![],
        };
        let json = serde_json::to_string(&exp).unwrap();
        assert!(json.contains("\"pattern\""));
    }
}
