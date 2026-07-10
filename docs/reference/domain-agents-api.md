# amplihack-domain-agents

API reference for the `amplihack-domain-agents` crate — specialized goal-seeking
agent implementations.

## Crate Overview

`amplihack-domain-agents` provides concrete agent implementations for specific
domains: teaching, code review, and meeting synthesis. All agents implement
the `DomainAgent` trait and integrate with the evaluation framework.

**Workspace dependency**: `amplihack-domain-agents = { path = "crates/amplihack-domain-agents" }`

## Modules

| Module                | Description                                      |
|-----------------------|--------------------------------------------------|
| `base`                | `DomainAgent` trait, `EvalLevel`, `EvalScenario` |
| `skill_injector`      | Dynamic capability injection                     |
| `teaching`            | `TeachingAgent` — Socratic pedagogy              |
| `code_review`         | `CodeReviewAgent` — security and logic review    |
| `code_synthesis`      | `CodeSynthesizer` — generation, refactor, analyze |
| `meeting_synthesizer` | `MeetingSynthesizerAgent` — transcript analysis  |

## Core Trait

### DomainAgent

```rust
pub trait DomainAgent: Send + Sync {
    /// Human-readable name for this agent type.
    fn name(&self) -> &str;

    /// Execute the agent's primary task on the given input.
    fn execute(&self, input: &str) -> Result<TaskResult, AgentError>;

    /// Evaluate the agent against a specific scenario.
    fn evaluate(&self, scenario: &EvalScenario) -> Result<TaskResult, AgentError>;

    /// Return the evaluation levels this agent supports.
    fn supported_levels(&self) -> &[EvalLevel];
}
```

### EvalLevel

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EvalLevel {
    L1,  // Basic recall
    L2,  // Factual QA
    L3,  // Cross-reference
    L4,  // Temporal reasoning
    L5,  // Causal reasoning
    L6,  // Teaching/explanation
    L7,  // Code generation
    L8,  // Self-improvement
    L9,  // Multi-agent coordination
    L10, // Long-horizon memory
    L11, // Meta-evaluation
    L12, // Full autonomy
}
```

### EvalScenario

```rust
pub struct EvalScenario {
    pub level: EvalLevel,
    pub input: String,
    pub expected_output: Option<String>,
    pub context: HashMap<String, Value>,
    pub timeout: Duration,
}
```

### TaskResult

```rust
pub struct TaskResult {
    pub output: String,
    pub confidence: f64,
    pub metadata: HashMap<String, Value>,
    pub memory_updates: Vec<MemoryEntry>,
}
```

### TeachingResult

```rust
pub struct TeachingResult {
    pub lesson_output: String,
    pub student_comprehension: f64,
    pub topics_covered: Vec<String>,
    pub follow_up_questions: Vec<String>,
}
```

## SkillInjector

```rust
pub struct SkillInjector {
    /* private fields */
}

impl SkillInjector {
    pub fn new() -> Self;
    pub fn register(&mut self, name: &str, skill: Box<dyn Skill>);
    pub fn unregister(&mut self, name: &str) -> Option<Box<dyn Skill>>;
    pub fn list(&self) -> Vec<String>;
    pub fn inject(&self, agent: Box<dyn DomainAgent>) -> Result<Box<dyn DomainAgent>, AgentError>;
}

pub trait Skill: Send + Sync {
    fn name(&self) -> &str;
    fn execute(&self, input: &str) -> Result<String, AgentError>;
    fn description(&self) -> &str;
}
```

## Teaching Agent

### TeachingConfig

```rust
pub struct TeachingConfig {
    pub subject: String,
    pub difficulty: Difficulty,
    pub model: String,
    pub memory: MemoryHandle,
    pub max_turns: u32,
    pub prompt_template: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum Difficulty {
    Beginner,
    Intermediate,
    Advanced,
    Expert,
}
```

### TeachingAgent

```rust
impl TeachingAgent {
    pub fn new(config: TeachingConfig) -> Self;
}

impl DomainAgent for TeachingAgent {
    fn name(&self) -> &str;
    fn execute(&self, input: &str) -> Result<TaskResult, AgentError>;
    fn evaluate(&self, scenario: &EvalScenario) -> Result<TaskResult, AgentError>;
    fn supported_levels(&self) -> &[EvalLevel]; // L1–L6
}
```

## Code Review Agent

### CodeReviewConfig

```rust
pub struct CodeReviewConfig {
    pub severity_threshold: Severity,
    pub categories: Vec<Category>,
    pub model: String,
    pub max_findings: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, Copy)]
pub enum Category {
    Security,
    Logic,
    Performance,
    Style,
    Documentation,
}
```

### CodeReviewAgent

```rust
impl CodeReviewAgent {
    pub fn new(config: CodeReviewConfig) -> Self;
    pub fn review_diff(&self, diff: &str) -> Result<Vec<Finding>, AgentError>;
}

pub struct Finding {
    pub severity: Severity,
    pub category: Category,
    pub file: String,
    pub line: Option<u32>,
    pub message: String,
    pub suggestion: Option<String>,
}

impl DomainAgent for CodeReviewAgent {
    fn name(&self) -> &str;
    fn execute(&self, input: &str) -> Result<TaskResult, AgentError>;
    fn evaluate(&self, scenario: &EvalScenario) -> Result<TaskResult, AgentError>;
    fn supported_levels(&self) -> &[EvalLevel]; // L1–L4
}
```

## Code Synthesizer

`CodeSynthesizer` provides deterministic code analysis and honest, typed errors
for code generation and refactoring. It does **not** fabricate placeholder code:
when no synthesis backend can honestly satisfy a request, it returns an explicit
`Err` rather than an `Ok` wrapping a stub. See the
[feature guide](../features/code-synthesis-honest-errors.md) and issue
[#874](https://github.com/rysweet/amplihack-rs/issues/874).

### Models

```rust
pub struct CodeSynthesisConfig {
    pub language: String,      // default: "rust"
    pub style: String,         // default: "idiomatic"
    pub max_complexity: u32,   // default: 10
}

pub struct CodeSpec {
    pub description: String,
    pub language: String,
    pub constraints: Vec<String>,
}

pub struct GeneratedCode {
    pub code: String,
    pub language: String,
    pub explanation: String,
}

pub struct CodeAnalysis {
    pub complexity: u32,
    pub issues: Vec<String>,
    pub suggestions: Vec<String>,
}
```

### CodeSynthesizer

```rust
impl CodeSynthesizer {
    pub fn new(config: CodeSynthesisConfig) -> Self;
    pub fn with_defaults() -> Self;
    pub fn config(&self) -> &CodeSynthesisConfig;

    /// Empty/whitespace description or language -> Err(DomainError::InvalidInput).
    /// Well-formed spec, no backend available   -> Err(DomainError::CodeSynthesis)
    ///   whose message interpolates the trimmed `spec.language`.
    /// Never returns Ok with stub/placeholder code.
    pub fn generate(&self, spec: &CodeSpec) -> Result<GeneratedCode>;

    /// Empty/whitespace code -> Err(DomainError::InvalidInput).
    /// Non-empty code, no backend available -> Err(DomainError::CodeSynthesis).
    /// Never returns Ok with stub/placeholder code.
    pub fn refactor(&self, code: &str) -> Result<GeneratedCode>;

    /// Deterministic heuristic. Returns Ok(CodeAnalysis) for all input.
    pub fn analyze(&self, code: &str) -> Result<CodeAnalysis>;
}
```

### Error contract

| Method     | Condition                                   | Returns |
| ---------- | ------------------------------------------- | ------- |
| `generate` | `description` or `language` empty/whitespace | `Err(DomainError::InvalidInput("code spec description and language must not be empty"))` |
| `generate` | well-formed spec, no backend                 | `Err(DomainError::CodeSynthesis("code synthesis backend not available: cannot synthesize <language>"))` |
| `refactor` | `code` empty/whitespace                      | `Err(DomainError::InvalidInput("code to refactor must not be empty"))` |
| `refactor` | non-empty `code`, no backend                 | `Err(DomainError::CodeSynthesis("refactoring backend not available"))` |
| `analyze`  | any input                                    | `Ok(CodeAnalysis { .. })` |

Error messages never interpolate `spec.description`, `spec.constraints`, or the
raw `code` body. For `generate`, the only caller-supplied value that appears is
the trimmed `spec.language` token (surrounding whitespace stripped, case
preserved).

## Meeting Synthesizer Agent

### MeetingSynthesizerAgent

```rust
pub struct MeetingSynthesizerConfig {
    pub model: String,
    pub extract_action_items: bool,
    pub extract_decisions: bool,
    pub extract_key_points: bool,
}

pub struct MeetingSummary {
    pub action_items: Vec<ActionItem>,
    pub decisions: Vec<Decision>,
    pub key_points: Vec<String>,
    pub participants: Vec<String>,
    pub duration_estimate: Option<Duration>,
}

pub struct ActionItem {
    pub description: String,
    pub assignee: Option<String>,
    pub deadline: Option<String>,
    pub priority: ActionPriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ActionPriority {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub description: String,
    pub participants: Vec<String>,
    pub rationale: Option<String>,
}

impl MeetingSynthesizerAgent {
    pub fn new(config: MeetingSynthesizerConfig) -> Self;
    pub fn synthesize(&self, transcript: &str) -> Result<MeetingSummary, AgentError>;
}

impl DomainAgent for MeetingSynthesizerAgent {
    fn name(&self) -> &str;
    fn execute(&self, input: &str) -> Result<TaskResult, AgentError>;
    fn evaluate(&self, scenario: &EvalScenario) -> Result<TaskResult, AgentError>;
    fn supported_levels(&self) -> &[EvalLevel]; // L1–L3
}
```

## Dependencies

| Crate                  | Purpose                    |
|------------------------|----------------------------|
| `amplihack-agent-core` | Base agent types           |
| `amplihack-memory`     | Memory integration         |
| `serde`                | Serialization              |
| `serde_json`           | JSON handling              |
| `thiserror`            | Error derive macros        |
| `tracing`              | Structured logging         |
