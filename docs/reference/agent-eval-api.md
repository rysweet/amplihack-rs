# amplihack-agent-eval

API reference for the `amplihack-agent-eval` crate — the progressive evaluation
framework for measuring agent capabilities.

## Crate Overview

`amplihack-agent-eval` provides the harness, graders, and evaluators for testing
agent performance across 12 progressive difficulty levels. Evaluation spans
process boundaries using persistent memory.

**Workspace dependency**: `amplihack-agent-eval = { path = "crates/amplihack-agent-eval" }`

## Modules

| Module                    | Description                                        |
|---------------------------|----------------------------------------------------|
| `harness`                 | `HarnessConfig`, `HarnessResult`, `run_harness`    |
| `grader`                  | `GradeResult`, `grade_answer`                      |
| `collector`               | `NewsArticle`, `collect_news`                      |
| `quiz`                    | `QuizQuestion`, `generate_quiz`                    |
| `subprocess`              | `AgentSubprocess` — isolated agent execution       |
| `teaching`                | `TeachingSession`, `TeachingConfig`, `TeachingResult` |
| `metacognition`           | `MetacognitionGrader`, `MetacognitionScore`        |
| `long_horizon`            | `LongHorizonMemoryEval`, `EvalReport`              |
| `long_horizon_self_improve` | `LongHorizonRunnerConfig`, `run_long_horizon_self_improve` |
| `general_capability`      | `GeneralCapabilityEval`, `CapabilityReport`        |
| `domain_eval`             | `DomainEvalHarness`, domain-specific evaluation    |
| `meta_eval`               | `MetaEvalExperiment`, `ExperimentConfig`            |
| `error`                   | `EvalError` enum                                   |

## Harness Runner

### HarnessConfig

```rust
pub struct HarnessConfig {
    pub news_file: PathBuf,
    pub output_dir: PathBuf,
    pub agent_name: String,
    pub memory_backend: Backend,
}
```

### HarnessResult

```rust
pub struct HarnessResult {
    pub success: bool,
    pub scores: Option<HashMap<String, f64>>,
    pub error_message: Option<String>,
}

impl HarnessResult {
    pub fn overall_score(&self) -> f64;
}
```

### run_harness

```rust
pub fn run_harness(config: &HarnessConfig) -> Result<HarnessResult, EvalError>;
```

Orchestrates the complete evaluation pipeline:
1. Collect test data from news file
2. Generate quiz questions at multiple levels
3. Run learning phase in isolated subprocess
4. Run testing phase in fresh subprocess
5. Grade answers against expected results
6. Produce scored report

## Grader

### grade_answer

```rust
pub fn grade_answer(
    question: &str,
    answer: &str,
    expected: &str,
) -> Result<GradeResult, EvalError>;
```

### GradeResult

```rust
pub struct GradeResult {
    pub score: f64,        // 0.0–1.0
    pub reasoning: String, // Explanation of the grade
    pub correct: bool,     // Binary pass/fail
}
```

## Multi-Source Collector

### NewsArticle

```rust
pub struct NewsArticle {
    pub url: String,
    pub title: String,
    pub content: String,
    pub published: String,
}
```

### collect_news

```rust
pub fn collect_news(websearch_data: &serde_json::Value) -> Result<Vec<NewsArticle>, EvalError>;
```

## Quiz Generator

### QuizQuestion

```rust
pub struct QuizQuestion {
    pub question: String,
    pub expected_answer: String,
    pub level: u32,          // 1–12
    pub source_urls: Vec<String>,
}
```

### generate_quiz

```rust
pub fn generate_quiz(articles: &[NewsArticle]) -> Result<Vec<QuizQuestion>, EvalError>;
```

## Teaching Evaluation

### TeachingConfig

```rust
pub struct TeachingConfig {
    pub subject: String,
    pub student_level: String,
    pub max_turns: u32,
    pub model: String,
}
```

### TeachingSession

```rust
impl TeachingSession {
    pub fn new(config: TeachingConfig) -> Self;
    pub fn run(&self) -> Result<TeachingResult, EvalError>;
}
```

### TeachingResult

```rust
pub struct TeachingResult {
    pub clarity: f64,
    pub engagement: f64,
    pub correctness: f64,
    pub turns: Vec<Turn>,
}

pub struct Turn {
    pub role: String,    // "teacher" or "student"
    pub content: String,
    pub turn_number: u32,
}
```

## Metacognition Grader

### MetacognitionGrader

```rust
impl MetacognitionGrader {
    pub fn new(model: &str) -> Self;
    pub fn grade(&self, response: &str) -> Result<MetacognitionScore, EvalError>;
}
```

### MetacognitionScore

```rust
pub struct MetacognitionScore {
    pub overall: f64,
    pub dimensions: HashMap<Dimension, f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dimension {
    Accuracy,
    Calibration,
    Uncertainty,
    Reflection,
    Adaptation,
    Transfer,
}
```

## Long-Horizon Memory Eval

### LongHorizonMemoryEval

```rust
pub struct LongHorizonConfig {
    pub total_turns: u32,
    pub checkpoint_interval: u32,
    pub categories: Vec<String>,
    pub model: String,
}

impl LongHorizonMemoryEval {
    pub fn new(config: LongHorizonConfig) -> Self;
    pub fn run(&self, agent: &dyn DomainAgent) -> Result<EvalReport, EvalError>;
}
```

### EvalReport

```rust
pub struct EvalReport {
    pub overall_retention: f64,
    pub category_breakdowns: Vec<CategoryBreakdown>,
    pub dimension_scores: Vec<DimensionScore>,
    pub results: Vec<EvalResult>,
}

pub struct CategoryBreakdown {
    pub category: String,
    pub retention_rate: f64,
    pub total_facts: u32,
    pub retained_facts: u32,
}

pub struct DimensionScore {
    pub dimension: String,
    pub score: f64,
}

pub struct EvalResult {
    pub turn: u32,
    pub question: String,
    pub answer: String,
    pub expected: String,
    pub score: f64,
}
```

## Long-Horizon Self-Improvement

### LongHorizonRunnerConfig

```rust
pub struct LongHorizonRunnerConfig {
    pub iterations: u32,
    pub eval_between_iterations: bool,
    pub improvement_strategy: String,
    pub model: String,
    pub agent_name: String,
    pub memory_backend: Backend,
}
```

### run_long_horizon_self_improve

```rust
pub fn run_long_horizon_self_improve(
    config: &LongHorizonRunnerConfig,
) -> Result<RunnerResult, EvalError>;

pub struct RunnerResult {
    pub iteration_scores: Vec<f64>,
    pub improvement_delta: f64,
    pub final_score: f64,
    pub strategies_tried: Vec<String>,
}
```

## General Capability Eval

### CapabilityEvalConfig

```rust
#[derive(Debug, Clone, Default)]
pub struct CapabilityEvalConfig {
    pub model: Option<String>,
    pub timeout: Option<Duration>,
    pub scenarios: Option<Vec<String>>,
}
```

### GeneralCapabilityEval

```rust
impl GeneralCapabilityEval {
    pub fn new(config: CapabilityEvalConfig) -> Self;
    pub fn run(&self, agent: &dyn DomainAgent) -> Result<CapabilityReport, EvalError>;
}
```

### CapabilityReport

```rust
pub struct CapabilityReport {
    pub tool_use: f64,
    pub planning: f64,
    pub reasoning: f64,
    pub transfer: f64,
    pub collaboration: f64,
    pub overall: f64,
    pub scenarios: Vec<ScenarioResult>,
}

pub struct ScenarioResult {
    pub name: String,
    pub capability: String,
    pub score: f64,
    pub tool_calls: Vec<ToolCall>,
}

pub struct ToolCall {
    pub tool: String,
    pub input: Value,
    pub output: Value,
    pub correct: bool,
}
```

## Domain Eval Harness

### DomainEvalHarness

```rust
impl DomainEvalHarness {
    pub fn new(agents: Vec<Box<dyn DomainAgent>>) -> Self;
    pub fn run_all(&self) -> Result<DomainEvalReport, EvalError>;
    pub fn run_agent(&self, agent: &dyn DomainAgent) -> Result<AgentEvalResult, EvalError>;
}

pub struct DomainEvalReport {
    pub results: Vec<AgentEvalResult>,
    pub timestamp: DateTime<Utc>,
}

pub struct AgentEvalResult {
    pub agent_name: String,
    pub score: f64,
    pub level_scores: HashMap<EvalLevel, f64>,
}
```

## Meta-Evaluation

### MetaEvalExperiment

```rust
pub struct ExperimentConfig {
    pub model: String,
    pub iterations: u32,
    pub eval_types: Vec<String>,
}

impl MetaEvalExperiment {
    pub fn new(config: ExperimentConfig) -> Self;
    pub fn run(&self) -> Result<ExperimentReport, EvalError>;
}

pub struct ExperimentReport {
    pub accuracy: f64,
    pub calibration: f64,
    pub consistency: f64,
    pub results_by_type: HashMap<String, f64>,
}
```

## EvalError

```rust
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("harness error: {0}")]
    Harness(String),
    #[error("grading error: {0}")]
    Grading(String),
    #[error("subprocess failed: {0}")]
    Subprocess(String),
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("data collection error: {0}")]
    Collection(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
```

## Dependencies

| Crate                  | Purpose                      |
|------------------------|------------------------------|
| `amplihack-agent-core` | Agent types and subprocess   |
| `amplihack-memory`     | Persistent memory for eval   |
| `serde`                | Serialization                |
| `serde_json`           | JSON output                  |
| `thiserror`            | Error derives                |
| `tracing`              | Structured logging           |
| `chrono`               | Timestamps                   |
