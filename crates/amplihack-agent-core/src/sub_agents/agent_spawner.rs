//! Agent Spawner: Dynamic sub-agent creation for complex tasks.
//!
//! Spawned agents share read access to the parent's memory via the shared
//! `parent_memory_path`. Results flow back through [`AgentSpawner::collect_results`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use regex::Regex;
use tracing::{info, warn};

use crate::error::{AgentError, Result};

use super::types::{SpawnedAgent, SpawnedAgentStatus, SpecialistType};

/// Executor function signature: takes a spawned agent, returns result or error.
type ExecutorFn = Box<dyn Fn(&SpawnedAgent) -> std::result::Result<String, String> + Send + Sync>;

struct ClassificationRule { keywords: &'static [&'static str], specialist: SpecialistType }

/// Order matters: more specific patterns checked first.
static CLASSIFICATION_RULES: &[ClassificationRule] = &[
    ClassificationRule { keywords: &["research", "web search", "look up online"], specialist: SpecialistType::Research },
    ClassificationRule { keywords: &["generate", "write code", "script", "implement", "create program", "create a program"], specialist: SpecialistType::CodeGeneration },
    ClassificationRule { keywords: &["combine", "synthesize", "summarize", "merge", "integrate"], specialist: SpecialistType::Synthesis },
    ClassificationRule { keywords: &["analyze", "pattern", "detect", "compare", "trend", "correlation"], specialist: SpecialistType::Analysis },
    ClassificationRule { keywords: &["find", "search", "retrieve", "lookup", "get facts", "what do we know"], specialist: SpecialistType::Retrieval },
];

// ---------------------------------------------------------------------------
// AgentSpawner
// ---------------------------------------------------------------------------

/// Factory for creating and managing specialist sub-agents.
/// ```
/// use amplihack_agent_core::sub_agents::AgentSpawner;
///
/// let mut spawner = AgentSpawner::new("parent", "/data/memory", "mini", 4).unwrap();
/// let _agent = spawner.spawn("Find all facts about Sarah", "auto").unwrap();
/// let results = spawner.collect_results(std::time::Duration::from_secs(30));
/// assert_eq!(results.len(), 1);
/// ```
pub struct AgentSpawner {
    parent_agent_name: String,
    parent_memory_path: String,
    #[allow(dead_code)]
    sdk_type: String,
    #[allow(dead_code)]
    max_concurrent: usize,
    spawned: Vec<SpawnedAgent>,
    spawn_counter: usize,
    executors: HashMap<SpecialistType, ExecutorFn>,
}

impl AgentSpawner {
    /// Create a new spawner for the given parent agent.
    pub fn new(
        parent_agent_name: &str,
        parent_memory_path: &str,
        sdk_type: &str,
        max_concurrent: usize,
    ) -> Result<Self> {
        let name = parent_agent_name.trim();
        if name.is_empty() {
            return Err(AgentError::ConfigError(
                "parent_agent_name cannot be empty".into(),
            ));
        }

        let mut executors: HashMap<SpecialistType, ExecutorFn> = HashMap::new();
        executors.insert(SpecialistType::Retrieval, Box::new(default_retrieval));
        executors.insert(SpecialistType::Analysis, Box::new(default_analysis));
        executors.insert(SpecialistType::Synthesis, Box::new(default_synthesis));
        executors.insert(SpecialistType::CodeGeneration, Box::new(default_code_gen));
        executors.insert(SpecialistType::Research, Box::new(default_research));

        Ok(Self {
            parent_agent_name: name.to_string(),
            parent_memory_path: parent_memory_path.to_string(),
            sdk_type: sdk_type.to_string(),
            max_concurrent: max_concurrent.clamp(1, 16),
            spawned: Vec::new(),
            spawn_counter: 0,
            executors,
        })
    }

    /// Register a custom executor for a specialist type.
    pub fn register_executor(
        &mut self,
        specialist_type: SpecialistType,
        executor: impl Fn(&SpawnedAgent) -> std::result::Result<String, String> + Send + Sync + 'static,
    ) {
        self.executors.insert(specialist_type, Box::new(executor));
    }

    /// Spawn a sub-agent. Use `specialist_type = "auto"` for auto-classification.
    pub fn spawn(&mut self, task: &str, specialist_type: &str) -> Result<SpawnedAgent> {
        let task = task.trim();
        if task.is_empty() {
            return Err(AgentError::ConfigError("task cannot be empty".into()));
        }

        let st = if specialist_type == "auto" {
            self.classify_task(task)
        } else {
            specialist_type.parse::<SpecialistType>()?
        };

        self.spawn_counter += 1;
        let name = format!("{}_sub_{}_{}", self.parent_agent_name, self.spawn_counter, st);

        let spawned = SpawnedAgent {
            name: name.clone(),
            specialist_type: st,
            task: task.to_string(),
            parent_memory_path: self.parent_memory_path.clone(),
            result: None,
            status: SpawnedAgentStatus::Pending,
            error: String::new(),
            elapsed_seconds: 0.0,
            metadata: HashMap::new(),
        };

        self.spawned.push(spawned.clone());
        info!(name = %name, specialist = %st, "Spawned sub-agent");
        Ok(spawned)
    }

    /// Execute all pending agents and return the full list.
    pub fn collect_results(&mut self, _timeout: Duration) -> &[SpawnedAgent] {
        let pending_indices: Vec<usize> = self
            .spawned
            .iter()
            .enumerate()
            .filter(|(_, s)| s.status == SpawnedAgentStatus::Pending)
            .map(|(i, _)| i)
            .collect();

        if pending_indices.is_empty() {
            return &self.spawned;
        }

        for &i in &pending_indices {
            self.spawned[i].status = SpawnedAgentStatus::Running;
        }

        for &i in &pending_indices {
            let start = Instant::now();
            let snapshot = self.spawned[i].clone();

            match self.executors.get(&snapshot.specialist_type) {
                Some(exec) => match exec(&snapshot) {
                    Ok(result) => {
                        self.spawned[i].result = Some(result);
                        self.spawned[i].status = SpawnedAgentStatus::Completed;
                    }
                    Err(err) => {
                        self.spawned[i].error = err;
                        self.spawned[i].status = SpawnedAgentStatus::Failed;
                    }
                },
                None => {
                    self.spawned[i].error =
                        format!("No executor for type: {}", snapshot.specialist_type);
                    self.spawned[i].status = SpawnedAgentStatus::Failed;
                    warn!(specialist = %snapshot.specialist_type, "No executor registered");
                }
            }
            self.spawned[i].elapsed_seconds = start.elapsed().as_secs_f64();
        }

        &self.spawned
    }

    /// Number of pending (not yet executed) agents.
    pub fn get_pending_count(&self) -> usize {
        self.spawned
            .iter()
            .filter(|s| s.status == SpawnedAgentStatus::Pending)
            .count()
    }

    /// Return only completed agents.
    pub fn get_completed_results(&self) -> Vec<&SpawnedAgent> {
        self.spawned
            .iter()
            .filter(|s| s.status == SpawnedAgentStatus::Completed)
            .collect()
    }

    /// Clear all spawned agents (reset state).
    pub fn clear(&mut self) {
        self.spawned.clear();
    }

    /// Auto-detect specialist type from task description using keyword matching.
    fn classify_task(&self, task: &str) -> SpecialistType {
        let task_lower = task.to_lowercase();

        for rule in CLASSIFICATION_RULES {
            for &kw in rule.keywords {
                if kw.contains(' ') {
                    // Multi-word: substring match
                    if task_lower.contains(kw) {
                        return rule.specialist;
                    }
                } else {
                    // Single-word: word-boundary match
                    let pattern = format!(r"\b{}\b", regex::escape(kw));
                    if let Ok(re) = Regex::new(&pattern)
                        && re.is_match(&task_lower)
                    {
                        return rule.specialist;
                    }
                }
            }
        }

        SpecialistType::Retrieval
    }
}

// ---------------------------------------------------------------------------
// Default executor implementations (placeholders)
// ---------------------------------------------------------------------------

fn default_retrieval(agent: &SpawnedAgent) -> std::result::Result<String, String> {
    Ok(format!("Retrieval task queued: {} (memory integration not configured)", agent.task))
}

fn default_analysis(agent: &SpawnedAgent) -> std::result::Result<String, String> {
    Ok(format!("Analysis task queued: {} (memory integration not configured)", agent.task))
}

fn default_synthesis(agent: &SpawnedAgent) -> std::result::Result<String, String> {
    Ok(format!("Synthesis task queued: {} (memory integration not configured)", agent.task))
}

fn default_code_gen(agent: &SpawnedAgent) -> std::result::Result<String, String> {
    let tl = agent.task.to_lowercase();
    if tl.contains("script") || tl.contains("python") {
        Ok(format!("# Generated script for: {}\ndef main():\n    pass\n", agent.task))
    } else {
        Ok(format!("Code generation for: {} (requires LLM integration)", agent.task))
    }
}

fn default_research(agent: &SpawnedAgent) -> std::result::Result<String, String> {
    Ok(format!("Research task queued: {} (external search not implemented)", agent.task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_name() {
        assert!(AgentSpawner::new("", "/mem", "mini", 4).is_err());
        assert!(AgentSpawner::new("  ", "/mem", "mini", 4).is_err());
    }

    #[test]
    fn new_clamps_max_concurrent() {
        let s = AgentSpawner::new("p", "/m", "mini", 100).unwrap();
        assert_eq!(s.max_concurrent, 16);
        let s2 = AgentSpawner::new("p", "/m", "mini", 0).unwrap();
        assert_eq!(s2.max_concurrent, 1);
    }

    #[test]
    fn spawn_rejects_empty_task() {
        let mut s = AgentSpawner::new("p", "/m", "mini", 4).unwrap();
        assert!(s.spawn("", "retrieval").is_err());
        assert!(s.spawn("  ", "retrieval").is_err());
    }

    #[test]
    fn spawn_creates_pending_agent() {
        let mut s = AgentSpawner::new("parent", "/mem", "mini", 4).unwrap();
        let agent = s.spawn("Find facts about Sarah", "retrieval").unwrap();
        assert_eq!(agent.status, SpawnedAgentStatus::Pending);
        assert_eq!(agent.specialist_type, SpecialistType::Retrieval);
        assert!(agent.name.contains("parent_sub_1_retrieval"));
        assert_eq!(s.get_pending_count(), 1);
    }

    #[test]
    fn spawn_auto_classifies() {
        let mut s = AgentSpawner::new("p", "/m", "mini", 4).unwrap();

        let a1 = s.spawn("Analyze the trend data", "auto").unwrap();
        assert_eq!(a1.specialist_type, SpecialistType::Analysis);

        let a2 = s.spawn("Generate a Python script", "auto").unwrap();
        assert_eq!(a2.specialist_type, SpecialistType::CodeGeneration);

        let a3 = s.spawn("Research the latest papers", "auto").unwrap();
        assert_eq!(a3.specialist_type, SpecialistType::Research);

        let a4 = s.spawn("Summarize the findings", "auto").unwrap();
        assert_eq!(a4.specialist_type, SpecialistType::Synthesis);

        let a5 = s.spawn("Find related documents", "auto").unwrap();
        assert_eq!(a5.specialist_type, SpecialistType::Retrieval);
    }

    #[test]
    fn classify_defaults_to_retrieval() {
        let s = AgentSpawner::new("p", "/m", "mini", 4).unwrap();
        assert_eq!(s.classify_task("some random task"), SpecialistType::Retrieval);
    }

    #[test]
    fn classify_multi_word_keyword() {
        let s = AgentSpawner::new("p", "/m", "mini", 4).unwrap();
        assert_eq!(s.classify_task("Please web search for info"), SpecialistType::Research);
        assert_eq!(s.classify_task("write code for the parser"), SpecialistType::CodeGeneration);
    }

    #[test]
    fn collect_results_executes_pending() {
        let mut s = AgentSpawner::new("parent", "/m", "mini", 4).unwrap();
        s.spawn("Find facts", "retrieval").unwrap();
        s.spawn("Analyze patterns", "analysis").unwrap();

        let results = s.collect_results(Duration::from_secs(10));
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.status == SpawnedAgentStatus::Completed));
        assert!(results[0].result.is_some());
        assert!(results[0].elapsed_seconds >= 0.0);
    }

    #[test]
    fn collect_results_skips_already_completed() {
        let mut s = AgentSpawner::new("p", "/m", "mini", 4).unwrap();
        s.spawn("task", "retrieval").unwrap();
        s.collect_results(Duration::from_secs(5));

        // Second collect should not re-execute
        let results = s.collect_results(Duration::from_secs(5));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, SpawnedAgentStatus::Completed);
    }

    #[test]
    fn get_completed_results() {
        let mut s = AgentSpawner::new("p", "/m", "mini", 4).unwrap();
        s.spawn("task1", "retrieval").unwrap();
        s.spawn("task2", "analysis").unwrap();
        s.collect_results(Duration::from_secs(5));

        let completed = s.get_completed_results();
        assert_eq!(completed.len(), 2);
    }

    #[test]
    fn clear_resets_state() {
        let mut s = AgentSpawner::new("p", "/m", "mini", 4).unwrap();
        s.spawn("task", "retrieval").unwrap();
        assert_eq!(s.get_pending_count(), 1);
        s.clear();
        assert_eq!(s.get_pending_count(), 0);
    }

    #[test]
    fn register_custom_executor() {
        let mut s = AgentSpawner::new("p", "/m", "mini", 4).unwrap();
        s.register_executor(SpecialistType::Retrieval, |agent| {
            Ok(format!("Custom: {}", agent.task))
        });
        s.spawn("task", "retrieval").unwrap();
        let results = s.collect_results(Duration::from_secs(5));
        assert_eq!(results[0].result.as_deref(), Some("Custom: task"));
    }

    #[test]
    fn code_gen_python_template() {
        let mut s = AgentSpawner::new("p", "/m", "mini", 4).unwrap();
        s.spawn("Write a Python script for parsing", "code_generation")
            .unwrap();
        let results = s.collect_results(Duration::from_secs(5));
        let result = results[0].result.as_deref().unwrap();
        assert!(result.contains("def main"));
        assert!(result.contains("Generated script"));
    }

    #[test]
    fn spawn_increments_counter() {
        let mut s = AgentSpawner::new("p", "/m", "mini", 4).unwrap();
        let a1 = s.spawn("task1", "retrieval").unwrap();
        let a2 = s.spawn("task2", "retrieval").unwrap();
        assert!(a1.name.contains("sub_1"));
        assert!(a2.name.contains("sub_2"));
    }
}
