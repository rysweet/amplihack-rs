//! Knowledge Builder orchestrator.
//!
//! Ported from `amplihack/knowledge_builder/orchestrator.py`.
//!
//! Implements the Socratic-method knowledge building pipeline:
//!
//! 1. **Question generation** — produce initial + follow-up questions about a topic.
//! 2. **Knowledge acquisition** — answer each question (e.g. via web search).
//! 3. **Artifact generation** — render the populated knowledge graph to files.
//!
//! Each step is defined as a trait so that callers can supply their own LLM-backed
//! or mock implementations.

use std::path::{Path, PathBuf};

use crate::kb_types::{KnowledgeGraph, Question};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during a knowledge-builder run.
#[derive(Debug, thiserror::Error)]
pub enum KnowledgeBuilderError {
    /// A pipeline step failed.
    #[error("pipeline step failed: {0}")]
    PipelineStep(String),
    /// I/O error (e.g. creating the output directory).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Pipeline traits
// ---------------------------------------------------------------------------

/// Generates questions about a topic using the Socratic method.
pub trait QuestionGenerator: Send + Sync {
    /// Produce all questions (initial + follow-ups) for `topic`.
    ///
    /// # Errors
    ///
    /// Returns an error if question generation fails.
    fn generate_all_questions(&self, topic: &str) -> Result<Vec<Question>, KnowledgeBuilderError>;
}

/// Acquires answers for a set of questions.
pub trait KnowledgeAcquirer: Send + Sync {
    /// Answer every question in `questions`, returning an updated vector with
    /// the `answer` field populated.
    ///
    /// # Errors
    ///
    /// Returns an error if knowledge acquisition fails.
    fn answer_all_questions(
        &self,
        questions: Vec<Question>,
        topic: &str,
    ) -> Result<Vec<Question>, KnowledgeBuilderError>;
}

/// Generates artefact files from a completed knowledge graph.
pub trait ArtifactGenerator: Send + Sync {
    /// Write artefacts for `graph` and return the paths of generated files.
    ///
    /// # Errors
    ///
    /// Returns an error if artifact generation fails.
    fn generate_all(&self, graph: &KnowledgeGraph) -> Result<Vec<PathBuf>, KnowledgeBuilderError>;
}

// ---------------------------------------------------------------------------
// Slug helper
// ---------------------------------------------------------------------------

/// Sanitise a topic string into a file-system-safe slug.
///
/// Keeps only ASCII alphanumerics, spaces, hyphens, and underscores from the
/// first 50 characters, then lowercases and replaces spaces with underscores.
pub fn topic_slug(topic: &str) -> String {
    topic
        .chars()
        .take(50)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .replace(' ', "_")
        .to_lowercase()
}

// ---------------------------------------------------------------------------
// KnowledgeBuilder
// ---------------------------------------------------------------------------

/// Configuration for a [`KnowledgeBuilder`] run.
pub struct KnowledgeBuilderConfig {
    /// The topic to research.
    pub topic: String,
    /// Agent command (e.g. `"claude"`). Falls back to `AMPLIHACK_AGENT_BINARY`
    /// environment variable, then to `"claude"`.
    pub agent_cmd: String,
    /// Root directory under which topic-specific output is placed.
    pub output_base: PathBuf,
}

impl KnowledgeBuilderConfig {
    /// Create a config with sensible defaults.
    ///
    /// `topic` is trimmed; `agent_cmd` is read from the environment if `None`.
    pub fn new(
        topic: impl Into<String>,
        agent_cmd: Option<String>,
        output_base: Option<PathBuf>,
    ) -> Self {
        let topic = topic.into().trim().to_string();
        let agent_cmd = agent_cmd
            .or_else(|| std::env::var("AMPLIHACK_AGENT_BINARY").ok())
            .unwrap_or_else(|| "claude".into());
        let output_base = output_base.unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".claude")
                .join("data")
        });
        Self {
            topic,
            agent_cmd,
            output_base,
        }
    }

    /// Resolved output directory (base + topic slug).
    pub fn output_dir(&self) -> PathBuf {
        self.output_base.join(topic_slug(&self.topic))
    }
}

/// Main orchestrator for the Knowledge Builder workflow.
///
/// Generic over the three pipeline steps so callers can inject real or mock
/// implementations.
pub struct KnowledgeBuilder<Q, A, G>
where
    Q: QuestionGenerator,
    A: KnowledgeAcquirer,
    G: ArtifactGenerator,
{
    config: KnowledgeBuilderConfig,
    question_gen: Q,
    knowledge_acq: A,
    artifact_gen: G,
}

impl<Q, A, G> KnowledgeBuilder<Q, A, G>
where
    Q: QuestionGenerator,
    A: KnowledgeAcquirer,
    G: ArtifactGenerator,
{
    /// Construct a new builder.
    pub fn new(
        config: KnowledgeBuilderConfig,
        question_gen: Q,
        knowledge_acq: A,
        artifact_gen: G,
    ) -> Self {
        Self {
            config,
            question_gen,
            knowledge_acq,
            artifact_gen,
        }
    }

    /// The resolved output directory for this run.
    pub fn output_dir(&self) -> PathBuf {
        self.config.output_dir()
    }

    /// The topic being researched.
    pub fn topic(&self) -> &str {
        &self.config.topic
    }

    /// The agent command string.
    pub fn agent_cmd(&self) -> &str {
        &self.config.agent_cmd
    }

    /// Execute the complete Socratic knowledge-building workflow.
    ///
    /// 1. Generate questions  
    /// 2. Answer questions  
    /// 3. Generate artefacts  
    ///
    /// Returns the output directory on success.
    ///
    /// # Errors
    ///
    /// Returns [`KnowledgeBuilderError`] if any pipeline step fails.
    pub fn build(&self) -> Result<PathBuf, KnowledgeBuilderError> {
        let topic = &self.config.topic;

        // Initialise an empty knowledge graph.
        let mut kg = KnowledgeGraph {
            topic: topic.clone(),
            questions: Vec::new(),
            triplets: Vec::new(),
            sources: Vec::new(),
            timestamp: String::new(),
        };

        // Step 1: generate questions.
        kg.questions = self.question_gen.generate_all_questions(topic)?;

        // Step 2: answer questions via knowledge acquisition.
        kg.questions = self
            .knowledge_acq
            .answer_all_questions(kg.questions, topic)?;

        // Step 3: generate artefacts.
        let _artifact_files = self.artifact_gen.generate_all(&kg)?;

        Ok(self.config.output_dir())
    }
}

/// Create a [`KnowledgeBuilderConfig`] from minimal arguments.
///
/// This mirrors the Python `KnowledgeBuilder.__init__` convenience API.
pub fn create_config(
    topic: &str,
    agent_cmd: Option<String>,
    output_base: Option<&Path>,
) -> KnowledgeBuilderConfig {
    KnowledgeBuilderConfig::new(topic, agent_cmd, output_base.map(|p| p.to_path_buf()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/knowledge_builder_tests.rs"]
mod tests;
