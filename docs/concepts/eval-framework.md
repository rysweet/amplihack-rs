# Evaluation Framework

The amplihack-rs evaluation framework (`amplihack-agent-eval`) provides a
progressive, multi-dimensional evaluation system for measuring agent
capabilities across execution boundaries.

## Design Philosophy

- **Evidence-based**: Real benchmark data, never guesswork
- **Progressive**: Levels L1–L12 test increasingly complex capabilities
- **Cross-boundary**: Evaluation spans subprocess restarts using persistent memory
- **Deterministic harness**: The harness itself is deterministic; only the agent
  under test introduces nondeterminism

## Architecture

```
┌─────────────────────────────────────────────────┐
│                 Harness Runner                    │
│  (orchestrates learn → test → grade pipeline)    │
├─────────┬──────────┬──────────┬─────────────────┤
│ Collector│ Quiz Gen │  Grader  │ Report Builder  │
│ (news)   │ (levels) │ (scoring)│ (JSON + human)  │
└─────────┴──────────┴──────────┴─────────────────┘
         ↕                ↕
┌─────────────────┐ ┌──────────────────┐
│ Agent Subprocess │ │  Memory Backend  │
│ (isolated exec)  │ │  (persistent)    │
└─────────────────┘ └──────────────────┘
```

## Evaluation Levels

| Level | Name                | What It Tests                                    |
|-------|---------------------|--------------------------------------------------|
| L1    | Basic Recall        | Can the agent remember and repeat facts?         |
| L2    | Factual QA          | Can it answer factual questions from memory?     |
| L3    | Cross-Reference     | Can it connect facts across multiple sources?    |
| L4    | Temporal Reasoning  | Does it understand time ordering of events?      |
| L5    | Causal Reasoning    | Can it identify cause-and-effect relationships?  |
| L6    | Teaching            | Can it explain concepts using Socratic method?   |
| L7    | Code Generation     | Can it produce working code from specifications? |
| L8    | Self-Improvement    | Does it improve its own prompts/strategies?      |
| L9    | Multi-Agent Coord.  | Can multiple agents collaborate effectively?     |
| L10   | Long-Horizon Memory | Does it retain knowledge across 1000+ turns?     |
| L11   | Meta-Evaluation     | Can it evaluate its own evaluation accuracy?     |
| L12   | Full Autonomy       | End-to-end task completion without guidance       |

## Quick Start

```rust
use amplihack_agent_eval::{HarnessConfig, run_harness};

let config = HarnessConfig {
    news_file: "test_data/news.json".into(),
    output_dir: "eval_results/".into(),
    agent_name: "my-agent".into(),
    memory_backend: "sqlite".into(),
};

let result = run_harness(&config)?;
println!("Success: {}, Score: {:.1}%",
    result.success,
    result.overall_score() * 100.0
);
```

## Harness Pipeline

The evaluation harness follows a fixed pipeline:

### 1. Collection

The `MultiSourceCollector` gathers test data from news feeds, generating
`NewsArticle` structs with URL, title, content, and publication date:

```rust
use amplihack_agent_eval::{collect_news, NewsArticle};

let articles: Vec<NewsArticle> = collect_news(&websearch_data)?;
```

### 2. Quiz Generation

The `QuizGenerator` produces questions at specified difficulty levels:

```rust
use amplihack_agent_eval::{generate_quiz, QuizQuestion};

let quiz: Vec<QuizQuestion> = generate_quiz(&articles)?;
for q in &quiz {
    println!("[L{}] {}", q.level, q.question);
}
```

### 3. Learning Phase (Subprocess)

The agent under test ingests content in an isolated subprocess:

```rust
use amplihack_agent_eval::AgentSubprocess;

let mut learner = AgentSubprocess::new(&config.agent_name)
    .memory_backend(&config.memory_backend);
learner.learn(&articles)?;
```

### 4. Testing Phase (Subprocess)

A fresh subprocess answers questions using only persistent memory:

```rust
let mut tester = AgentSubprocess::new(&config.agent_name)
    .memory_backend(&config.memory_backend);
let answers = tester.answer(&quiz)?;
```

### 5. Grading

The `Grader` scores answers against expected results:

```rust
use amplihack_agent_eval::{grade_answer, GradeResult};

let grade: GradeResult = grade_answer(&question, &answer, &expected)?;
println!("Score: {:.2}, Reasoning: {}", grade.score, grade.reasoning);
```

## Specialized Evaluators

### Metacognition Grader

Evaluates an agent's self-awareness across six dimensions:

```rust
use amplihack_agent_eval::{MetacognitionGrader, Dimension};

let grader = MetacognitionGrader::new("claude-sonnet-4-5");
let score = grader.grade(&agent_response)?;

for (dim, val) in &score.dimensions {
    println!("{:?}: {:.2}", dim, val);
}
// Dimensions: Accuracy, Calibration, Uncertainty, Reflection, Adaptation, Transfer
```

### Long-Horizon Memory Eval

Stress-tests memory retention across 1000+ turns:

```rust
use amplihack_agent_eval::{LongHorizonMemoryEval, EvalReport};

let eval = LongHorizonMemoryEval::new(LongHorizonConfig {
    total_turns: 1000,
    checkpoint_interval: 100,
    categories: vec!["facts", "procedures", "episodes"],
    ..Default::default()
});

let report: EvalReport = eval.run(&agent)?;
for breakdown in &report.category_breakdowns {
    println!("{}: {:.1}%", breakdown.category, breakdown.retention_rate * 100.0);
}
```

### Teaching Evaluator

Measures teaching effectiveness through simulated student interactions:

```rust
use amplihack_agent_eval::{TeachingSession, TeachingConfig, TeachingResult};

let session = TeachingSession::new(TeachingConfig {
    subject: "binary search".into(),
    student_level: "beginner".into(),
    max_turns: 20,
    model: "claude-sonnet-4-5".into(),
});

let result: TeachingResult = session.run()?;
println!("Clarity: {:.2}, Engagement: {:.2}", result.clarity, result.engagement);
```

### General Capability Eval

Tests five general agent capabilities:

```rust
use amplihack_agent_eval::{GeneralCapabilityEval, CapabilityReport};

let eval = GeneralCapabilityEval::new(Default::default());
let report: CapabilityReport = eval.run(&agent)?;

// Capabilities tested:
// - Tool Use: correct tool selection and invocation
// - Planning: multi-step task decomposition
// - Reasoning: logical inference chains
// - Transfer: applying knowledge to new domains
// - Collaboration: multi-agent coordination
```

## Self-Improvement Loop

The `LongHorizonSelfImprove` runner orchestrates iterative agent improvement:

```rust
use amplihack_agent_eval::{run_long_horizon_self_improve, LongHorizonRunnerConfig};

let config = LongHorizonRunnerConfig {
    iterations: 5,
    eval_between_iterations: true,
    improvement_strategy: "prompt-refinement".into(),
    ..Default::default()
};

let result = run_long_horizon_self_improve(&config)?;
for (i, score) in result.iteration_scores.iter().enumerate() {
    println!("Iteration {}: {:.1}%", i + 1, score * 100.0);
}
```

## Output Format

All evaluation results are written as JSON for machine consumption:

```json
{
  "agent_name": "my-agent",
  "timestamp": "2026-04-03T00:00:00Z",
  "overall_score": 0.847,
  "level_scores": {
    "L1": 0.95,
    "L2": 0.90,
    "L3": 0.85,
    "L4": 0.80
  },
  "dimension_scores": {
    "accuracy": 0.92,
    "calibration": 0.78,
    "retention": 0.85
  },
  "questions_answered": 50,
  "questions_correct": 42
}
```

## Related

- [Agent Lifecycle](./agent-lifecycle.md) — Agent state machine
- [Domain Agents](./domain-agents.md) — Specialized agent types
- [Memory Backend Architecture](./memory-backend-architecture.md) — Persistent memory for cross-boundary eval
